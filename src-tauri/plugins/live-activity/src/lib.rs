//! iOS-only bridge to the Swift side in `ios/Sources`: starts/updates/ends
//! the Lock Screen Live Activity, publishes the same state to the App Group
//! for the home-screen and Lock Screen widgets, and holds the background
//! keep-alive while an activity is running. A no-op on every other platform,
//! so callers never need their own `cfg`.

use serde::Serialize;
use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_live_activity);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("live-activity")
        .setup(|app, api| {
            #[cfg(target_os = "ios")]
            let state = LiveActivity {
                handle: api.register_ios_plugin(init_plugin_live_activity)?,
            };
            #[cfg(not(target_os = "ios"))]
            let state = {
                let _ = api;
                LiveActivity::<R> {
                    _runtime: std::marker::PhantomData,
                }
            };
            app.manage(state);
            Ok(())
        })
        .build()
}

pub struct LiveActivity<R: Runtime> {
    #[cfg(target_os = "ios")]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "ios"))]
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> LiveActivity<R> {
    /// Starts the Live Activity if none is running (only possible while the
    /// app is in the foreground -- an ActivityKit rule; a background attempt
    /// just fails and is retried on the next call), otherwise updates it.
    /// Also starts the background keep-alive. `content` must serialize to
    /// the Swift side's `UpdateArgs`.
    ///
    /// Blocks until the Swift side has handled the call -- run it off the
    /// async runtime (e.g. `spawn_blocking`).
    pub fn update<T: Serialize>(&self, content: &T) -> Result<(), String> {
        #[cfg(target_os = "ios")]
        return self
            .handle
            .run_mobile_plugin::<serde::de::IgnoredAny>("update", content)
            .map(|_| ())
            .map_err(|e| e.to_string());
        #[cfg(not(target_os = "ios"))]
        {
            let _ = content;
            Ok(())
        }
    }

    /// Ends any running Live Activity, marks the widgets idle, and releases
    /// the keep-alive so iOS can suspend the app normally again. Blocks like
    /// `update`.
    pub fn end(&self) -> Result<(), String> {
        #[cfg(target_os = "ios")]
        return self
            .handle
            .run_mobile_plugin::<serde::de::IgnoredAny>("end", ())
            .map(|_| ())
            .map_err(|e| e.to_string());
        #[cfg(not(target_os = "ios"))]
        Ok(())
    }
}

pub trait LiveActivityExt<R: Runtime> {
    fn live_activity(&self) -> &LiveActivity<R>;
}

impl<R: Runtime, T: Manager<R>> LiveActivityExt<R> for T {
    fn live_activity(&self) -> &LiveActivity<R> {
        self.state::<LiveActivity<R>>().inner()
    }
}
