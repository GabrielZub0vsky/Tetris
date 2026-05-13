//! Game configuration used for testing and user-initiated setup.

use bevy::prelude::Resource;
use bevy::time::{Timer, TimerMode};
use rand::SeedableRng;
use rand::rngs::SmallRng;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::{net::IpAddr, time::Duration};

use crate::{bag::*, data::*};

/// The default port to connect to the server
pub const DEFAULT_SERVER_PORT: u16 = 1337;
/// The default replication interval for the server
pub const DEFAULT_REPLICATION_INTERVAL_MS: u64 = 16;
/// The seed for the garbage RNG
const GARBAGE_SEED: u64 = 727;

fn default_send_garbage() -> bool {
    false
}

fn default_expected_players() -> usize {
    1
}

/// Game configuration to read from the user or from the tests.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Copy, Clone, Resource)]
pub struct GameConfig {
    /// The type of bag for this game (only one is supported per server because
    /// we declare GameConfig to be a resource for convenience).
    pub bag: BagType,
    /// Whether to animate the title text.
    pub animate_title: bool,
    /// The IP address of the server.
    pub server_addr: IpAddr,
    /// The port the networking server is serving from.
    pub server_port: u16,
    /// Server->client replication interval, in milliseconds
    pub replication_interval_ms: u64,
    /// Whether to send garbage when scoring
    #[serde(default = "default_send_garbage")]
    pub send_garbage: bool,
    /// The expected number of players
    #[serde(default = "default_expected_players")]
    pub expected_players: usize,
}

impl GameConfig {
    /// Build an initial shared game state based on this configuration.
    pub fn build_shared_game_state(&self) -> SharedGameState {
        SharedGameState {
            manual_drop_gravity: SOFT_DROP_GRAVITY,
            score: 0,
            lines_cleared: 0,
            lines_cleared_since_last_level: 0,
            level: 0,
            gravity_timer: Timer::new(
                SharedGameState::initial_drop_interval(),
                TimerMode::Repeating,
            ),
            hard_drop: false,
            client_id: 0,
        }
    }
    /// Build an initial private game state based on this configuration.
    pub fn build_private_game_state(&self) -> PrivateGameState {
        PrivateGameState {
            bag: self.bag.create_bag(),
            send_garbage: self.send_garbage,
            garbage_rng: SmallRng::seed_from_u64(GARBAGE_SEED),
        }
    }

    /// Read a configuration from given JSON data.
    pub fn load(json: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let cfg: Self = serde_json::from_str(json)?;
        assert!(cfg.expected_players > 0 && cfg.expected_players <= 3);
        Ok(cfg)
    }

    /// Replication interval as a Rust Duration.
    pub fn replication_interval(&self) -> Duration {
        Duration::from_millis(self.replication_interval_ms)
    }
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            bag: Default::default(),
            animate_title: true,
            server_addr: Ipv4Addr::from_octets([127, 0, 0, 1]).into(),
            server_port: DEFAULT_SERVER_PORT,
            replication_interval_ms: DEFAULT_REPLICATION_INTERVAL_MS,
            send_garbage: false,
            expected_players: 1,
        }
    }
}

/// What type of bag to create in the initial state.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Default)]
pub enum BagType {
    /// A deterministic bag that cycles through all tetrominos
    Deterministic,
    /// A randomized bag with a given starting random seed
    FixedSeed(u64),
    /// A randomized bag with a seed picked at runtime
    #[default]
    RandomSeed,
}

impl BagType {
    /// Create a new bag based on the parameters specified by this object.
    pub fn create_bag(&self) -> Box<dyn Bag + Sync> {
        use BagType::*;

        match self {
            Deterministic => Box::new(DeterministicBag::default()),
            FixedSeed(seed) => Box::new(RandomBag::from_seed(*seed)),
            RandomSeed => Box::new(RandomBag::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[test]
    fn default_game_state() {
        let cfg = GameConfig {
            bag: BagType::Deterministic,
            ..Default::default()
        };
        let state = cfg.build_shared_game_state();
        let priv_state = cfg.build_private_game_state();

        assert!((priv_state.bag.as_ref() as &dyn Any).is::<DeterministicBag>());
        assert_eq!(state.hard_drop, false);
        assert_eq!(state.manual_drop_gravity, SOFT_DROP_GRAVITY);
        assert_eq!(state.score, 0);
        assert_eq!(state.lines_cleared, 0);
        assert_eq!(state.lines_cleared_since_last_level, 0);
        assert_eq!(state.level, 0);
        assert_eq!(
            state.gravity_timer.duration(),
            SharedGameState::initial_drop_interval()
        );
        assert_eq!(state.gravity_timer.mode(), TimerMode::Repeating);
    }

    #[test]
    fn bag_creation() {
        let cfg = GameConfig {
            bag: BagType::Deterministic,
            ..Default::default()
        };
        let state = cfg.build_private_game_state();

        assert!((state.bag.as_ref() as &dyn Any).is::<DeterministicBag>());

        let cfg = GameConfig {
            bag: BagType::FixedSeed(727),
            ..Default::default()
        };
        let state = cfg.build_private_game_state();

        assert!((state.bag.as_ref() as &dyn Any).is::<RandomBag>());
        assert_eq!(
            (state.bag.as_ref() as &dyn Any).downcast_ref::<RandomBag>(),
            Some(&RandomBag::from_seed(727))
        );

        let cfg = GameConfig {
            bag: BagType::RandomSeed,
            ..Default::default()
        };
        let state1 = cfg.build_private_game_state();
        let state2 = cfg.build_private_game_state();

        assert!((state1.bag.as_ref() as &dyn Any).is::<RandomBag>());
        assert!((state2.bag.as_ref() as &dyn Any).is::<RandomBag>());
        assert_ne!(
            (state1.bag.as_ref() as &dyn Any).downcast_ref::<RandomBag>(),
            (state2.bag.as_ref() as &dyn Any).downcast_ref::<RandomBag>()
        );
    }

    #[test]
    fn load() {
        assert_eq!(
            GameConfig::load(r#"{"bag":"Deterministic","animate_title":true,"server_addr":"127.0.0.1","server_port":1337,"replication_interval_ms":16}"#).unwrap(),
            GameConfig {
                bag: BagType::Deterministic,
                ..Default::default()
            }
        );

        assert_eq!(
            GameConfig::load(r#"{"bag":"Deterministic","animate_title":false,"server_addr":"127.0.0.1","server_port":1337,"replication_interval_ms":16}"#).unwrap(),
            GameConfig {
                bag: BagType::Deterministic,
                animate_title: false,
                ..Default::default()
            }
        );
    }

    #[test]
    fn load2() {
        assert_eq!(
            GameConfig::load(r#"{"bag":"RandomSeed","animate_title":true,"server_addr":"127.0.0.1","server_port":1337,"replication_interval_ms":16}"#).unwrap(),
            GameConfig {
                bag: BagType::RandomSeed,
                ..Default::default()
            }
        );

        assert_eq!(
            GameConfig::load(r#"{"bag":{"FixedSeed": 272},"animate_title":true,"server_addr":"127.0.0.1","server_port":1337,"replication_interval_ms":16}"#).unwrap(),
            GameConfig {
                bag: BagType::FixedSeed(272),
                ..Default::default()
            }
        );

        assert_eq!(
            GameConfig::load(r#"{"bag":{"FixedSeed": 727},"animate_title":true,"server_addr":"127.0.0.1","server_port":1337,"replication_interval_ms":16}"#).unwrap(),
            GameConfig {
                bag: BagType::FixedSeed(727),
                ..Default::default()
            }
        );

        assert_eq!(
            GameConfig::load(r#"{"bag":{"FixedSeed": 727},"animate_title":true,"server_addr":"1.2.3.4","server_port":1453,"replication_interval_ms":17}"#).unwrap(),
            GameConfig {
                bag: BagType::FixedSeed(727),
                server_addr: Ipv4Addr::from_octets([1, 2, 3, 4]).into(),
                server_port: 1453,
                replication_interval_ms: 17,
                animate_title: true,
                send_garbage: false,
                expected_players: 1,
            }
        );

        assert_eq!(
            GameConfig::load(r#"{"bag":{"FixedSeed": 727},"animate_title":true,"server_addr":"1.2.3.4","server_port":1453,"replication_interval_ms":17,"send_garbage":true}"#).unwrap(),
            GameConfig {
                bag: BagType::FixedSeed(727),
                server_addr: Ipv4Addr::from_octets([1, 2, 3, 4]).into(),
                server_port: 1453,
                replication_interval_ms: 17,
                animate_title: true,
                send_garbage: true,
                expected_players: 1,
            }
        );

        assert_eq!(
            GameConfig::load(r#"{"bag":{"FixedSeed": 727},"animate_title":true,"server_addr":"1.2.3.4","server_port":1453,"replication_interval_ms":17,"send_garbage":false,"expected_players":3}"#).unwrap(),
            GameConfig {
                bag: BagType::FixedSeed(727),
                server_addr: Ipv4Addr::from_octets([1, 2, 3, 4]).into(),
                server_port: 1453,
                replication_interval_ms: 17,
                animate_title: true,
                send_garbage: true,
                expected_players: 3,
            }
        );
    }
}
