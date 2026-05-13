//! The main server binary

use ::server::net::ServerPlugin;
use bevy::prelude::*;
use clap::Parser;
use common::FIXED_TIMESTEP_HZ;
use common::config::GameConfig;
use common::protocol::ProtocolPlugin;
use core::time::Duration;
use lightyear::prelude::server::ServerPlugins;

/// Command-line arguments
#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the JSON config file
    #[arg(short, long)]
    pub config: Option<String>,
}

fn main() {
    let args = Args::parse();
    let cfg = if let Some(config_file) = args.config {
        let json =
            String::try_from(std::fs::read(&config_file).expect("could not read the config file"))
                .expect("the config file's contents are not proper UTF-8.");
        GameConfig::load(&json).expect("could not parse the config file")
    } else {
        GameConfig::default()
    };

    App::new()
        .add_plugins((MinimalPlugins, bevy::log::LogPlugin::default()))
        .add_plugins(ServerPlugins {
            // How often we want the server to run a "tick"
            tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
        })
        .insert_resource(cfg)
        .add_plugins(ProtocolPlugin)
        .add_plugins(ServerPlugin)
        .run();
}
