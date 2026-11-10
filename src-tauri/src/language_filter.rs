//! Subtitle-driven language filter creation: downloads captions from
//! SubDL (or scans a local `.srt`/`.vtt` file), scans for profanity/blasphemy
//! from an editable word list, and writes a `FilterList`-compatible JSON
//! filter file in the format the app expects.
//!
//! This is a port of `scripts/build_language_filter.py` into Rust,
//! exposing Tauri commands so the frontend can orchestrate the same
//! pipeline: pick a title + year, download subtitles, scan, merge,
//! and save the resulting filter file.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::filter::{self, CueAction, FilterList, MediaEntry};
use crate::library;

// --------------------------------------------------------------------------
// Constants
// --------------------------------------------------------------------------

const SUBDL_SEARCH_URL: &str = "https://api.subdl.com/api/v1/subtitles";
const SUBDL_LINK_PREFIX: &str = "https://dl.subdl.com";
const USER_AGENT: &str = "family-filter-caption-scanner/1.0";

/// The wordlist JSON embedded at compile time — works on all platforms
/// (desktop, iOS simulator, iOS device) without any filesystem access.
const EMBEDDED_WORDLIST: &str = include_str!("../../scripts/wordlists/language_filter_words.json");

/// Comic-strip grawlix symbols -- must stay identical to the Python
/// script's GRAWLIX (same set, same order) so a word masked here and a word
/// masked by the app's `censorWord` look the same.
const GRAWLIX: &[char] = &['#', '@', '&', '!', '*', '%'];

// --------------------------------------------------------------------------
// Data structures
// --------------------------------------------------------------------------

/// A subtitle line parsed from an SRT/VTT file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleEntry {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// A cue (time range + action + category + masked word).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub action: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word: Option<String>,
}

/// Configuration for the scan pipeline.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    pub action: String,
    pub pad: f64,
    pub merge_gap: f64,
    pub min_severity: String,
    pub no_words: bool,
}

/// Options for downloading a subtitle from SubDL.
#[derive(Debug, Clone)]
pub struct SubdlOptions {
    pub api_key: String,
    pub title: String,
    pub year: Option<i32>,
    pub language: String,
    pub release_filter: Option<String>,
    pub sources: usize,
    pub cross_check_tolerance: f64,
}

/// Information about a SubDL search result (used to show results to the user).
#[derive(Debug, Clone, Serialize)]
pub struct SubdlResultInfo {
    pub index: usize,
    pub release_name: String,
    pub language: String,
    pub author: String,
}

// --------------------------------------------------------------------------
// Regex patterns
// --------------------------------------------------------------------------

static TIME_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(\d{2}):(\d{2}):(\d{2})[,.](\d{3})\s*-->\s*(\d{2}):(\d{2}):(\d{2})[,.](\d{3})")
        .unwrap()
});

static TAG_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"<[^>]+>|\{[^}]*\}").unwrap());

static NON_WORD_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"[^a-z0-9'\s]").unwrap());

static WHITESPACE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\s+").unwrap());

// --------------------------------------------------------------------------
// Core logic (pure functions, no I/O)
// --------------------------------------------------------------------------

/// Port of the Python `_timecode_to_seconds`: parses HH:MM:SS.mmm into
/// fractional seconds.
fn timecode_to_seconds(h: i32, m: i32, s: i32, ms: i32) -> f64 {
    h as f64 * 3600.0 + m as f64 * 60.0 + s as f64 + ms as f64 / 1000.0
}

/// Parse SRT/VTT text into subtitle entries.
pub fn parse_subtitles(raw: &str) -> Vec<SubtitleEntry> {
    let text = raw.replace("\r\n", "\n").replace("\r", "\n");
    let mut entries: Vec<SubtitleEntry> = Vec::new();

    // Split on blank lines
    let re_split = Regex::new(r"\n\s*\n").unwrap();
    for block in re_split.split(text.trim()) {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }

        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();

        for (i, line) in lines.iter().enumerate() {
            if let Some(cap) = TIME_RE.captures(line) {
                let h1: i32 = cap[1].parse().unwrap_or(0);
                let m1: i32 = cap[2].parse().unwrap_or(0);
                let s1: i32 = cap[3].parse().unwrap_or(0);
                let ms1: i32 = cap[4].parse().unwrap_or(0);
                let h2: i32 = cap[5].parse().unwrap_or(0);
                let m2: i32 = cap[6].parse().unwrap_or(0);
                let s2: i32 = cap[7].parse().unwrap_or(0);
                let ms2: i32 = cap[8].parse().unwrap_or(0);

                let start = timecode_to_seconds(h1, m1, s1, ms1);
                let end = timecode_to_seconds(h2, m2, s2, ms2);

                let body: String = lines[i + 1..].join("\n");
                if !body.trim().is_empty() {
                    entries.push(SubtitleEntry {
                        start,
                        end,
                        text: body,
                    });
                }
                break;
            }
        }
    }

    entries
}

/// Port of the Python `normalize`: strips subtitle tags, lowercases,
/// removes punctuation, collapses whitespace.
pub fn normalize(text: &str) -> String {
    let text = TAG_RE.replace_all(text, " ");
    let text = text.to_lowercase();
    let text = NON_WORD_RE.replace_all(&text, " ");
    WHITESPACE_RE.replace_all(&text, " ").trim().to_string()
}

/// Port of the Python `censor_word`: first letter kept, remaining
/// characters replaced with grawlix symbols keyed off each character's
/// code point.
pub fn censor_word(word: &str) -> String {
    fn censor_one(w: &str) -> String {
        if w.len() <= 1 {
            return w.to_string();
        }
        let mut result = String::new();
        let chars: Vec<char> = w.chars().collect();
        result.push(chars[0]);
        for ch in &chars[1..] {
            let idx = (*ch as usize) % GRAWLIX.len();
            result.push(GRAWLIX[idx]);
        }
        result
    }

    word.split(' ')
        .map(censor_one)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Severity ranking: mild < moderate < strong.
fn severity_rank(severity: &str) -> u8 {
    match severity {
        "mild" => 0,
        "moderate" => 1,
        "strong" => 2,
        _ => 0,
    }
}

/// A compiled matcher for a single word/phrase in the wordlist.
#[derive(Clone)]
pub struct Matcher {
    pub normalized_word: String,
    pub pattern: Regex,
}

/// Load the wordlist JSON and return matchers for profanity/blasphemy,
/// filtered by minimum severity.
pub fn load_matchers(
    wordlist_content: &str,
    min_severity: &str,
) -> Result<Vec<(String, Vec<Matcher>)>> {
    let wordlist: serde_json::Value =
        serde_json::from_str(wordlist_content).context("failed to parse wordlist JSON")?;

    let min_rank = severity_rank(min_severity);
    let mut all_matchers: Vec<(String, Vec<Matcher>)> = Vec::new();

    for category in &["profanity", "blasphemy"] {
        let entries = wordlist
            .get(category)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut matchers: Vec<Matcher> = Vec::new();
        for entry in entries {
            let severity = entry
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("mild");

            if severity_rank(severity) < min_rank {
                continue;
            }

            // Try word_b64 first, fall back to plain "word"
            let word = if let Some(b64) = entry.get("word_b64").and_then(|v| v.as_str()) {
                STANDARD
                    .decode(b64.as_bytes())
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default()
            } else if let Some(w) = entry.get("word").and_then(|v| v.as_str()) {
                w.to_string()
            } else {
                continue;
            };

            if word.is_empty() {
                continue;
            }

            let normalized = normalize(&word);
            if normalized.is_empty() {
                continue;
            }

            // Word-boundary pattern for the normalized word
            let pattern = Regex::new(&format!("\\b{}\\b", regex::escape(&normalized)))
                .with_context(|| format!("failed to compile pattern for {}", word))?;

            matchers.push(Matcher {
                normalized_word: normalized,
                pattern,
            });
        }

        if !matchers.is_empty() {
            all_matchers.push((category.to_string(), matchers));
        }
    }

    Ok(all_matchers)
}

/// Scan subtitle entries against the given matchers, returning cues
/// for every match found.
pub fn scan_entries(entries: &[SubtitleEntry], matchers: &[(String, Vec<Matcher>)]) -> Vec<Cue> {
    let mut hits: Vec<Cue> = Vec::new();

    for entry in entries {
        let normalized = normalize(&entry.text);
        if normalized.is_empty() {
            continue;
        }

        for (category, patterns) in matchers {
            for matcher in patterns {
                if matcher.pattern.is_match(&normalized) {
                    let masked = censor_word(&matcher.normalized_word);
                    hits.push(Cue {
                        start: entry.start,
                        end: entry.end,
                        action: "mute".to_string(),
                        category: format!("language-{}", category),
                        word: Some(masked),
                    });
                }
            }
        }
    }

    hits
}

/// Combine word lists from two cues, in first-seen order without
/// duplicates.
fn combine_words(a: &str, b: &str) -> String {
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }

    let mut words: Vec<String> = a.split(" / ").map(|s| s.to_string()).collect();
    for word in b.split(" / ") {
        if !words.contains(&word.to_string()) {
            words.push(word.to_string());
        }
    }
    words.join(" / ")
}

/// Port of `merge_cues`: pad each cue, merge overlapping/near cues
/// within the same category.
pub fn merge_cues(cues: &[Cue], pad: f64, merge_gap: f64, action: &str) -> Vec<Cue> {
    // Group by category
    let mut by_category: HashMap<String, Vec<Cue>> = HashMap::new();
    for cue in cues {
        by_category
            .entry(cue.category.clone())
            .or_default()
            .push(cue.clone());
    }

    let mut merged: Vec<Cue> = Vec::new();

    for (category, mut group) in by_category {
        group.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut current: Option<Cue> = None;

        for cue in group {
            let start = (cue.start - pad).max(0.0);
            let end = cue.end + pad;

            if current.is_none() {
                current = Some(Cue {
                    start,
                    end,
                    action: action.to_string(),
                    category: category.clone(),
                    word: cue.word,
                });
                continue;
            }

            let current_mut = current.as_mut().unwrap();
            if start <= current_mut.end + merge_gap {
                current_mut.end = current_mut.end.max(end);
                current_mut.word = Some(combine_words(
                    current_mut.word.as_deref().unwrap_or(""),
                    cue.word.as_deref().unwrap_or(""),
                ));
            } else {
                merged.push(current_mut.clone());
                current = Some(Cue {
                    start,
                    end,
                    action: action.to_string(),
                    category: category.clone(),
                    word: cue.word,
                });
            }
        }

        if let Some(c) = current {
            merged.push(c);
        }
    }

    merged.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged
}

// --------------------------------------------------------------------------
// SubDL API integration
// --------------------------------------------------------------------------

/// Parse a JSON response from the SubDL search API.
#[derive(Debug, Deserialize)]
struct SubdlSearchResponse {
    status: Option<bool>,
    error: Option<String>,
    message: Option<String>,
    subtitles: Option<Vec<SubdlResult>>,
}

#[derive(Debug, Deserialize)]
struct SubdlResult {
    url: Option<String>,
    release_name: Option<String>,
    name: Option<String>,
    lang: Option<String>,
    author: Option<String>,
}

/// Search SubDL for subtitles, returning matched result info.
pub async fn subdl_search_list(
    api_key: &str,
    title: &str,
    year: Option<i32>,
    language: &str,
    release_filter: Option<&str>,
) -> Result<(Vec<SubdlResultInfo>, Vec<SubdlResult>)> {
    let mut query: HashMap<String, String> = HashMap::new();
    query.insert("api_key".to_string(), api_key.to_string());
    query.insert("film_name".to_string(), title.to_string());
    query.insert("type".to_string(), "movie".to_string());
    query.insert("languages".to_string(), language.to_string());
    query.insert("subs_per_page".to_string(), "30".to_string());
    if let Some(y) = year {
        query.insert("year".to_string(), y.to_string());
    }

    let resp = reqwest::Client::new()
        .get(SUBDL_SEARCH_URL)
        .header("User-Agent", USER_AGENT)
        .query(&query)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("SubDL search request failed: {}", e))?;

    let status_code = resp.status().as_u16();
    let body = resp
        .bytes()
        .await
        .context("failed to read SubDL response body")?;

    if body.is_empty() {
        bail!("SubDL returned empty response");
    }

    let data: SubdlSearchResponse =
        serde_json::from_slice(&body).context("failed to parse SubDL JSON")?;

    let is_ok = data.status.unwrap_or(false);
    let error_msg = data
        .error
        .or(data.message)
        .unwrap_or_else(|| "unknown error".to_string());

    if status_code != 200 || !is_ok {
        bail!(
            "SubDL search failed (status={}): {}",
            status_code,
            error_msg
        );
    }

    let subtitles = data
        .subtitles
        .ok_or_else(|| anyhow::anyhow!("SubDL response missing subtitles field"))?;

    if subtitles.is_empty() {
        bail!(
            "No subtitles found for '{}' (year={:?}, lang={})",
            title,
            year,
            language
        );
    }

    // Apply release filter if given
    let filtered: Vec<SubdlResult> = if let Some(pattern) = release_filter {
        let re = Regex::new(&format!(r"(?i){}", pattern))?;
        subtitles
            .into_iter()
            .filter(|s| {
                let haystack = format!(
                    "{} {}",
                    s.release_name.as_deref().unwrap_or(""),
                    s.name.as_deref().unwrap_or("")
                );
                re.is_match(&haystack)
            })
            .collect()
    } else {
        subtitles
    };

    if filtered.is_empty() {
        bail!(
            "{} subtitle(s) found for '{}' but none matched '{}'. Pass empty release filter to consider all.",
            filtered.len(),
            title,
            release_filter.unwrap_or("")
        );
    }

    let infos: Vec<SubdlResultInfo> = filtered
        .iter()
        .enumerate()
        .map(|(i, s)| SubdlResultInfo {
            index: i,
            release_name: s
                .release_name
                .as_deref()
                .or(s.name.as_deref())
                .unwrap_or("?")
                .to_string(),
            language: s.lang.as_deref().unwrap_or("?").to_string(),
            author: s.author.as_deref().unwrap_or("?").to_string(),
        })
        .collect();

    Ok((infos, filtered))
}

/// Download subtitle content from a SubDL URL. Handles both zip and raw
/// files.
pub async fn subdl_download(download_url: &str) -> Result<String> {
    let resp = reqwest::Client::new()
        .get(download_url)
        .header("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("SubDL download failed: {}", e))?;

    let content = resp
        .bytes()
        .await
        .context("failed to read subtitle download")?;

    // Check if it's a zip
    if content.len() >= 2 && &content[0..2] == b"PK" {
        // Read from zip
        let mut archive =
            zip::ZipArchive::new(io::Cursor::new(content)).context("failed to open zip archive")?;
        let mut found: Option<String> = None;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_lowercase();
            if name.ends_with(".srt") || name.ends_with(".vtt") {
                let mut contents = String::new();
                entry.read_to_string(&mut contents)?;
                // Prefer .srt over .vtt
                if name.ends_with(".srt") {
                    return Ok(contents);
                }
                found = Some(contents);
            }
        }

        if let Some(s) = found {
            return Ok(s);
        }
        bail!("Downloaded zip contained no .srt/.vtt file");
    }

    // Try UTF-8 first, fall back to Latin-1
    String::from_utf8(content.to_vec())
        .map_err(|e| anyhow::anyhow!("not valid UTF-8: {}", e))
        .or_else(|_| {
            // Latin-1 decode: every byte is valid
            Ok(String::from_utf8_lossy(&content).to_string())
        })
        .map_err(|e: io::Error| anyhow::anyhow!("subtitle content decode failed: {}", e))
}

// --------------------------------------------------------------------------
// Slug helper
// --------------------------------------------------------------------------

/// Convert a title to a filesystem-safe slug (lowercase, hyphens, no
/// accents).
pub fn slugify(value: &str) -> String {
    // Normalize accents to ASCII
    let normalized: String = value
        .chars()
        .flat_map(|c| {
            c.to_lowercase()
                .filter(|ch| ch.is_ascii())
                .collect::<Vec<_>>()
        })
        .into_iter()
        .collect();

    let slug: String = normalized
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .to_lowercase();

    // Collapse multiple hyphens and strip leading/trailing
    slug.split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

// --------------------------------------------------------------------------
// Full pipeline: one movie
// --------------------------------------------------------------------------

/// Build a `FilterList` for a single movie title.
///
/// This is the core pipeline: download/parse subtitles -> scan for
/// profanity/blasphemy -> merge cues -> produce a `FilterList`.
pub async fn build_language_filter(
    title: &str,
    year: Option<i32>,
    service: &str,
    config: &ScanConfig,
    wordlist_content: &str,
    options: Option<&SubdlOptions>,
    srt_path: Option<&std::path::Path>,
) -> Result<FilterList> {
    // Load matchers
    let matchers = load_matchers(wordlist_content, &config.min_severity)?;

    // Get subtitle entries
    let (entries, entries_source): (Vec<SubtitleEntry>, String) = if let Some(srt) = srt_path {
        let raw = fs::read_to_string(srt)
            .with_context(|| format!("failed to read SRT file: {}", srt.display()))?;
        let parsed = parse_subtitles(&raw);
        if parsed.is_empty() {
            bail!("no subtitle cues could be parsed from {}", srt.display());
        }
        (parsed, srt.display().to_string())
    } else if let Some(opts) = options {
        // Download from SubDL
        if opts.api_key.is_empty() {
            bail!("SubDL API key is required (set --api-key or SUBDL_API_KEY)");
        }

        let release_filter = opts
            .release_filter
            .as_ref()
            .filter(|f| !f.is_empty())
            .map(|f| f.as_str());

        let results = subdl_search_list(
            &opts.api_key,
            title,
            opts.year,
            &opts.language,
            release_filter,
        )
        .await
        .context("SubDL search failed")?;

        let (_infos, subs) = results;
        if subs.is_empty() {
            bail!("no subtitle results found for '{}'", title);
        }

        // Use top-N sources, or just top-1
        let count = opts.sources.max(1).min(subs.len());
        let top_subs = &subs[..count];

        let mut all_entries: Vec<SubtitleEntry> = Vec::new();
        for (i, sub) in top_subs.iter().enumerate() {
            let download_url = format!("{}{}", SUBDL_LINK_PREFIX, sub.url.as_deref().unwrap_or(""));

            let release_name = sub
                .release_name
                .as_deref()
                .unwrap_or(sub.name.as_deref().unwrap_or(title))
                .to_string();

            println!("  Using subtitle match {}: {}", i, release_name);

            let raw = subdl_download(&download_url)
                .await
                .context("SubDL download failed")?;
            let parsed = parse_subtitles(&raw);
            if parsed.is_empty() {
                eprintln!(
                    "warning: no subtitle cues parsed from '{}', skipping",
                    release_name
                );
                continue;
            }
            println!(
                "  Parsed {} subtitle lines from {}",
                parsed.len(),
                release_name
            );
            all_entries.extend(parsed);
        }

        if all_entries.is_empty() {
            bail!("no subtitle source yielded parseable cues");
        }

        (all_entries, format!("{} (SubDL)", title))
    } else {
        bail!("must provide either --srt-file or a SubDL API key");
    };

    println!(
        "Parsed {} subtitle entries from {}",
        entries.len(),
        entries_source
    );

    // Scan for matches
    let hits = scan_entries(&entries, &matchers);
    println!("Found {} raw hits before merging.", hits.len());

    // Merge cues
    let merged = merge_cues(&hits, config.pad, config.merge_gap, &config.action);
    println!("{} cues after merging.", merged.len());

    // Build FilterList
    let mut media_entry = MediaEntry {
        title: title.to_string(),
        service: service.to_string(),
        cues: merged
            .into_iter()
            .map(|c| filter::Cue {
                start: c.start,
                end: c.end,
                action: if c.action == "skip" {
                    CueAction::Skip
                } else {
                    CueAction::Mute
                },
                category: c.category,
                word: c.word,
                note: None,
            })
            .collect(),
        confirmed: false,
        release_year: year.map(|y| y.to_string()).unwrap_or_default(),
    };

    // If --no-words, clear all cue words
    if config.no_words {
        for cue in &mut media_entry.cues {
            cue.word = None;
        }
    }

    media_entry.sort_and_validate()?;

    Ok(FilterList {
        media: vec![media_entry],
    })
}

// --------------------------------------------------------------------------
// Write filter file
// --------------------------------------------------------------------------

/// Write a `FilterList` to a JSON file, using the compact cue format
/// (each cue on one line) matching what `build_language_filter.py` writes.
pub fn write_filter_file(output_path: &std::path::Path, filter_list: &FilterList) -> Result<()> {
    let json =
        serde_json::to_string_pretty(filter_list).context("failed to serialize filter list")?;

    let parent = output_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .filter(|p| p.as_os_str() != ".");

    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::File::create(output_path).context("failed to create output file")?;
    writeln!(file, "{}", json)?;

    Ok(())
}

/// Write a `FilterList` to a slugified JSON file in the given directory.
pub fn write_filter_to_dir(
    base_dir: &std::path::Path,
    title: &str,
    service: &str,
    year: Option<i32>,
    filter_list: &FilterList,
) -> Result<PathBuf> {
    let slug = slugify(title);
    let output_path = base_dir.join(format!("{}.json", slug));
    write_filter_file(&output_path, filter_list)?;
    Ok(output_path)
}

// --------------------------------------------------------------------------
// Tauri command handlers
// --------------------------------------------------------------------------

/// Preview subtitle search results from SubDL (shows what would be
/// downloaded before actually downloading).
#[tauri::command]
pub async fn preview_subdl_results(
    title: String,
    year: Option<i32>,
    language: String,
    release_filter: Option<String>,
    api_key: String,
) -> Result<serde_json::Value, String> {
    if api_key.is_empty() {
        return Err("SubDL API key is required".to_string());
    }

    let release_filter_ref = release_filter.as_deref().filter(|f| !f.is_empty());

    let results = subdl_search_list(&api_key, &title, year, &language, release_filter_ref)
        .await
        .map_err(|e| e.to_string())?;

    let (infos, _subs) = results;

    Ok(serde_json::json!({
        "results": infos,
        "count": infos.len()
    }))
}

/// Download subtitles from SubDL and return the raw text.
#[tauri::command]
pub async fn download_subtitles(
    title: String,
    year: Option<i32>,
    language: String,
    release_filter: Option<String>,
    api_key: String,
) -> Result<serde_json::Value, String> {
    if api_key.is_empty() {
        return Err("SubDL API key is required".to_string());
    }

    let release_filter_ref = release_filter.as_deref().filter(|f| !f.is_empty());

    let results = subdl_search_list(&api_key, &title, year, &language, release_filter_ref)
        .await
        .map_err(|e| e.to_string())?;

    let (_infos, subs) = results;
    if subs.is_empty() {
        return Err("no subtitle results found".to_string());
    }

    // Download the top result
    let download_url = format!(
        "{}{}",
        SUBDL_LINK_PREFIX,
        subs[0].url.as_deref().unwrap_or("")
    );

    let raw = subdl_download(&download_url)
        .await
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "subtitle_text": raw,
        "release_name": subs[0].release_name,
        "language": subs[0].lang
    }))
}

/// Scan subtitle text for profanity/blasphemy using the embedded wordlist.
#[tauri::command]
pub async fn scan_subtitles(
    subtitle_text: String,
    _wordlist_path: PathBuf,
    min_severity: String,
) -> Result<serde_json::Value, String> {
    let matchers = load_matchers(EMBEDDED_WORDLIST, &min_severity).map_err(|e| e.to_string())?;

    let entries = parse_subtitles(&subtitle_text);

    if entries.is_empty() {
        return Err("no subtitle cues found in the provided text".to_string());
    }

    let hits = scan_entries(&entries, &matchers);

    Ok(serde_json::json!({
        "total_entries": entries.len(),
        "hits": hits.len(),
        "hit_cues": hits
    }))
}

/// Build a complete language filter from a SubDL download or local SRT file.
#[tauri::command]
pub async fn build_filter_from_subdl(
    title: String,
    service: String,
    year: Option<i32>,
    api_key: String,
    language: String,
    _wordlist_path: Option<String>,
    release_filter: Option<String>,
    min_severity: String,
    action: String,
    pad: f64,
    merge_gap: f64,
    no_words: bool,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("SubDL API key is required".to_string());
    }

    let config = ScanConfig {
        action,
        pad,
        merge_gap,
        min_severity,
        no_words,
    };

    let options = SubdlOptions {
        api_key,
        title: title.clone(),
        year,
        language,
        release_filter,
        sources: 1,
        cross_check_tolerance: 1.5,
    };

    let filter_list = build_language_filter(
        &title,
        year,
        &service,
        &config,
        EMBEDDED_WORDLIST,
        Some(&options),
        None,
    )
    .await
    .map_err(|e| e.to_string())?;

    let json = serde_json::to_string_pretty(&filter_list).map_err(|e| e.to_string())?;

    Ok(json)
}

/// Build a language filter from a local SRT/VTT file.
#[tauri::command]
pub async fn build_filter_from_srt(
    title: String,
    service: String,
    srt_path: String,
    _wordlist_path: Option<String>,
    min_severity: String,
    action: String,
    pad: f64,
    merge_gap: f64,
    no_words: bool,
) -> Result<String, String> {
    let config = ScanConfig {
        action,
        pad,
        merge_gap,
        min_severity,
        no_words,
    };

    let filter_list = build_language_filter(
        &title,
        None, // year not used for local SRT
        &service,
        &config,
        EMBEDDED_WORDLIST,
        None,
        Some(PathBuf::from(&srt_path).as_path()),
    )
    .await
    .map_err(|e| e.to_string())?;

    let json = serde_json::to_string_pretty(&filter_list).map_err(|e| e.to_string())?;

    Ok(json)
}

/// Build and save a language filter to disk.
#[tauri::command]
pub async fn build_and_save_filter(
    title: String,
    service: String,
    year: Option<i32>,
    api_key: String,
    language: String,
    _wordlist_path: Option<String>,
    release_filter: Option<String>,
    min_severity: String,
    action: String,
    pad: f64,
    merge_gap: f64,
    no_words: bool,
    output_dir: String,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("SubDL API key is required".to_string());
    }

    let config = ScanConfig {
        action,
        pad,
        merge_gap,
        min_severity,
        no_words,
    };

    let options = SubdlOptions {
        api_key,
        title: title.clone(),
        year,
        language,
        release_filter,
        sources: 1,
        cross_check_tolerance: 1.5,
    };

    let filter_list = build_language_filter(
        &title,
        year,
        &service,
        &config,
        EMBEDDED_WORDLIST,
        Some(&options),
        None,
    )
    .await
    .map_err(|e| e.to_string())?;

    let output_path = write_filter_to_dir(
        &PathBuf::from(&output_dir),
        &title,
        &service,
        year,
        &filter_list,
    )
    .map_err(|e| e.to_string())?;

    Ok(output_path.to_string_lossy().to_string())
}

/// Build a language filter from SubDL subtitles and add it to the
/// local filter library — downloads captions, scans for profanity/
/// blasphemy, writes the JSON file, and registers it in the library
/// so it immediately appears in the Select Filter grid.
#[tauri::command]
pub async fn build_and_add_filter(
    title: String,
    service: String,
    year: Option<i32>,
    api_key: String,
    language: String,
    _wordlist_path: Option<String>,
    release_filter: Option<String>,
    min_severity: String,
    action: String,
    pad: f64,
    merge_gap: f64,
    no_words: bool,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("SubDL API key is required".to_string());
    }

    let config = ScanConfig {
        action,
        pad,
        merge_gap,
        min_severity,
        no_words,
    };

    let options = SubdlOptions {
        api_key: api_key.clone(),
        title: title.clone(),
        year,
        language,
        release_filter,
        sources: 1,
        cross_check_tolerance: 1.5,
    };

    let filter_list = build_language_filter(
        &title,
        year,
        &service,
        &config,
        EMBEDDED_WORDLIST,
        Some(&options),
        None,
    )
    .await
    .map_err(|e| e.to_string())?;

    let output_dir = crate::paths::data_dir().join(library::IMPORTED_FILTERS_DIR);
    let output_path = write_filter_to_dir(&output_dir, &title, &service, year, &filter_list)
        .map_err(|e| e.to_string())?;

    // Register in the local filter library so it shows up in the grid.
    library::register_filter_path(&output_path).map_err(|e| e.to_string())?;

    Ok(output_path.to_string_lossy().to_string())
}
