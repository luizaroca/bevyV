//! Bevy Extend: additions to Bevy that live in this fork.
//!
//! Everything this fork adds goes in `bevy_extend*` crates so that pulling in upstream Bevy
//! changes rarely conflicts with our work. Add [`ExtendPlugins`] to an app to enable them.

pub mod ssgi;

use bevy_app::{plugin_group, Plugin};

plugin_group! {
    /// All Bevy Extend plugins.
    pub struct ExtendPlugins {
        :ExtendCorePlugin,
        ssgi:::ScreenSpaceGlobalIlluminationPlugin,
    }
}

/// Placeholder for shared setup; feature plugins are added to [`ExtendPlugins`] as they land.
#[derive(Default)]
pub struct ExtendCorePlugin;

impl Plugin for ExtendCorePlugin {
    fn build(&self, _app: &mut bevy_app::App) {}
}
