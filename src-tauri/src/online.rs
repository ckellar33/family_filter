//! Browsing (never writing -- see `db/schema.sql`'s doc comment) the online
//! filter library: a Neon Postgres table, read over a direct Postgres
//! connection as a dedicated `filter_reader` role that can only ever
//! `SELECT` rows with `status = 'approved'` (see `db/schema.sql`'s Row
//! Level Security policy for that role) -- so the connection string below
//! is safe to embed in every build the same way `metadata.rs`'s TMDB key
//! is: extracting it from the binary gets you nothing but read access to
//! data this screen already shows on request.
//!
//! Originally built against Neon's Data API (a PostgREST-shaped HTTP
//! endpoint, which would have meant zero database credential anywhere in
//! the app at all) -- abandoned once it turned out every request there,
//! including ones meant for the public "anonymous" role, requires a real
//! signed JWT from a registered auth provider, and Neon's own docs on
//! registering a *self-signed* provider (hosting a JWKS, claim mapping)
//! were too thin to build against with confidence. A direct connection to a
//! role Postgres itself restricts is the boring, fully-documented
//! alternative -- `rustls` (not `native-tls`/OpenSSL) for the same
//! cross-compilation reason `metadata.rs`'s `reqwest` uses it, so this
//! doesn't reintroduce the C-toolchain dependency that choice avoided.
//!
//! Publishing is a separate, deliberately out-of-band path
//! (`scripts/publish_filter.py`, run by hand with a private *owner*
//! connection string that's never checked in or shipped) -- see that
//! script's doc comment for why.
//!
//! Downloading a row hands it straight through `FilterList::load` (by
//! writing it to a real file first) rather than re-validating it by hand --
//! that's the same sort-and-check-every-invariant logic every other entry
//! point into this app's filter files already goes through, so a malformed
//! or tampered row is rejected exactly as it would be if a user had picked
//! a bad file off disk themselves.

use rustls::{ClientConfig, RootCertStore};
use tauri::AppHandle;
use tokio_postgres::Client;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::control::describe;
use crate::filter::{self, FilterList};
use crate::{library, metadata};

/// `filter_reader`'s connection string -- see this module's doc comment for
/// why it's safe to ship. Points at the same project/database
/// `scripts/publish_filter.py`'s (private, owner-role) connection string
/// does, just as the read-only role `db/schema.sql` creates instead of the
/// database owner.
const READER_CONNECTION_STRING: &str =
    "postgresql://filter_reader:oe0uk8jGoD1ZBVDb7XFDsT8xVYfG1F1S@ep-little-hall-b5r1zosv-pooler.c-7.us-east-2.aws.neon.tech/neondb?sslmode=require";

/// Same "drop a file next to the binary" override `metadata::tmdb_api_key`
/// uses -- lets this be pointed at a different branch/project (e.g. a
/// staging table) without a rebuild, on every platform that can write a
/// sidecar file. Not needed for the common case: every build already ships
/// with `READER_CONNECTION_STRING` pointed at the real project.
const READER_URL_STORE: &str = "neon_reader_url.store";

fn reader_connection_string() -> String {
    std::fs::read_to_string(crate::paths::data_dir().join(READER_URL_STORE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| READER_CONNECTION_STRING.to_string())
}

/// Opens a fresh connection as `filter_reader` -- one per call rather than
/// a pooled/shared client, since this is only ever used for the Select
/// Filter screen's "Online" grid load, not a hot path worth keeping a
/// standing connection open for. The connection's own background I/O task
/// is spawned onto the Tokio runtime and left to finish on its own once
/// `client` (and every clone of it) drops, same as any other
/// fire-and-forget task in this app.
async fn connect() -> Result<Client, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    let tls = MakeRustlsConnect::new(tls_config);

    let (client, connection) =
        tokio_postgres::connect(&reader_connection_string(), tls).await.map_err(|e| format!("couldn't reach the online filter library: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("[online] connection error: {e}");
        }
    });
    Ok(client)
}

/// One tile in the Select Filter screen's Online grid -- mirrors
/// `control::FilterTile`'s one-tile-per-title dedup (a title recorded on
/// more than one service, see `MediaEntry`'s doc comment, gets one poster,
/// not one per service), but carries `media` -- every service variant's raw
/// `MediaEntry` JSON for this title -- instead of a local `path`: there's no
/// file on disk for an online entry until `download_online_filter` creates
/// one, so this is everything that command needs to do that, round-tripped
/// back from the frontend rather than re-fetched.
#[derive(serde::Serialize)]
pub struct OnlineFilterTile {
    pub title: String,
    pub poster: Option<String>,
    /// From whichever service variant happens to be first -- same
    /// "whichever the dedupe kept" simplicity `list_filter_tiles`'s own
    /// cue-count badge already accepts for a multi-service local file.
    pub cue_count: usize,
    pub media: Vec<serde_json::Value>,
}

/// Every approved entry in the online library, grouped into one tile per
/// title, for the Select Filter screen's "Online" grid -- same poster-
/// lookup treatment `control::list_filter_tiles` gives local tiles (cached
/// TMDB search, see `metadata::poster_data_uri`), since the database itself
/// never stores cover art.
#[tauri::command]
pub async fn list_online_filters(app: AppHandle) -> Result<Vec<OnlineFilterTile>, String> {
    let client = connect().await?;
    // No `where status = 'approved'` here -- `filter_reader`'s Row Level
    // Security policy (db/schema.sql) already limits every query it can
    // possibly run to those rows, so this asks for everything it's allowed
    // to see. Ordered by service too so a title's variants group together
    // in a stable, predictable order (matters for which one ends up
    // "first" for cue_count below).
    let rows = client
        .query("select title, service, media from public.online_filters order by title asc, service asc", &[])
        .await
        .map_err(|e| format!("couldn't read the online filter library: {e}"))?;

    // Groups every service variant under its normalized title -- same
    // dedup key `control::list_filter_tiles` uses -- while remembering
    // first-seen order so the grid doesn't jump around release to release.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, (String, Vec<serde_json::Value>)> = std::collections::HashMap::new();
    for row in rows {
        let title: String = row.get(0);
        let media: serde_json::Value = row.get(2);

        // A row whose `media` this build's MediaEntry can't parse (e.g.
        // published by a newer app version with a field this one doesn't
        // know) just drops that one service variant -- same "skip, don't
        // fail the whole load" tolerance list_filter_tiles gives an
        // unloadable local file -- rather than losing every other variant
        // of the same title along with it. Goes through `media_entry_from_
        // value` (not a bare `serde_json::from_value::<MediaEntry>`) so a
        // row published in the newer "HH:MM:SS.ss" cue-time format doesn't
        // get mistaken for one of these and dropped -- see that function's
        // doc comment.
        if filter::media_entry_from_value(media.clone()).is_err() {
            continue;
        }

        let key = filter::normalize_title(&title);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_insert_with(|| (title, Vec::new())).1.push(media);
    }

    let mut tiles = Vec::new();
    for key in order {
        let (title, media) = groups.remove(&key).expect("just inserted");
        let cue_count = media
            .first()
            .and_then(|m| filter::media_entry_from_value(m.clone()).ok())
            .map(|e| e.cues.len())
            .unwrap_or(0);
        let poster = metadata::poster_data_uri(&app, &title).await;
        tiles.push(OnlineFilterTile { title, poster, cue_count, media });
    }
    Ok(tiles)
}

/// Saves one online tile's `media` -- every service variant of `title`
/// together -- as a new local filter file and registers it in the local
/// library, the same shape a hand-authored multi-service file already has
/// (e.g. sample-filters/captain-america-winter-soldier.json) -- after this,
/// the title shows up in "My Filters" exactly like anything added via
/// `add_filter_files`, service switcher included. Written to disk and
/// immediately re-loaded through `FilterList::load` rather than trusted as-
/// is: that's the same validation (sorted, non-overlapping, non-empty
/// category) every other filter file on this app goes through, and a file
/// that somehow fails it is deleted again rather than left behind as a tile
/// that can never actually open.
#[tauri::command]
pub fn download_online_filter(title: String, media: Vec<serde_json::Value>) -> Result<(), String> {
    let path = library::fresh_local_path(&format!("{title}.json")).map_err(|e| describe(&e))?;

    let doc = serde_json::json!({ "media": media });
    let json = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("failed to save {}: {e}", path.display()))?;

    let list = match FilterList::load(&path) {
        Ok(list) => list,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(format!("{title} isn't a valid filter entry: {}", describe(&e)));
        }
    };
    // Re-save through `FilterList::save` rather than trusting the row's
    // `media` JSON verbatim -- a row published before cue times switched to
    // "HH:MM:SS.ss" (see filter.rs's `seconds_to_hms`) still holds raw
    // seconds, and `load` above tolerates either format on read but doesn't
    // rewrite anything. Without this, a downloaded file could silently land
    // on disk in the old format while every locally-authored one is in the
    // new one. Best-effort: a failure here just leaves the file in whatever
    // format the row had, same as before this existed -- not worth failing
    // the whole download over.
    if let Err(e) = list.save(&path) {
        eprintln!("[online] failed to normalize downloaded filter {}: {e}", path.display());
    }

    library::register_filter_path(&path).map_err(|e| describe(&e))
}
