//! Next-tile bag implementations

use super::*;
use crate::data::*;
use rand::{SeedableRng, rngs::SmallRng, seq::SliceRandom};
use std::any::Any;

/// A generic interface for the next tile bag
pub trait Bag: Send + Any {
    /// Get the next tetromino and remove it from the bag
    fn next_tetromino(&mut self) -> Tetromino;
    /// Get the next tetromino without removing it from the bag
    fn peek(&mut self) -> Tetromino;
}

/// A deterministic tile bag that cycles between tetrominos.
#[derive(Default, Debug)]
pub struct DeterministicBag {
    next_tile_index: usize,
}

impl Bag for DeterministicBag {
    /// Take the next tetromino out of the bag.
    fn next_tetromino(&mut self) -> Tetromino {
        let t = self.peek();
        self.next_tile_index = (self.next_tile_index + 1) % ALL_TETROMINO_TYPES.len();
        t
    }

    /// Peek at the next tetromino.
    ///
    /// This function is `&mut self` in case the bag needs to be refilled.
    fn peek(&mut self) -> Tetromino {
        get_tetromino(ALL_TETROMINO_TYPES[self.next_tile_index])
    }
}

/// The random tile bag
#[derive(PartialEq, Debug)]
pub struct RandomBag {
    remaining_pieces: Vec<Tetromino>,
    rng: SmallRng,
}

impl RandomBag {
    /// Create a bag from given starting RNG seed.
    pub fn from_seed(seed: u64) -> Self {
        Self {
            remaining_pieces: vec![],
            rng: SmallRng::seed_from_u64(seed),
        }
    }

    fn refill(&mut self) {
        debug_assert!(self.remaining_pieces.is_empty());
        self.remaining_pieces
            .extend(ALL_TETROMINO_TYPES.into_iter().map(get_tetromino));
        self.remaining_pieces.shuffle(&mut self.rng);
    }
}

impl Bag for RandomBag {
    fn next_tetromino(&mut self) -> Tetromino {
        if let Some(cells) = self.remaining_pieces.pop() {
            cells
        } else {
            self.refill();
            self.next_tetromino()
        }
    }
    fn peek(&mut self) -> Tetromino {
        if self.remaining_pieces.is_empty() {
            self.refill();
        }
        *self.remaining_pieces.last().unwrap()
    }
}

impl Default for RandomBag {
    fn default() -> Self {
        Self {
            remaining_pieces: vec![],
            rng: SmallRng::from_os_rng(),
        }
    }
}
