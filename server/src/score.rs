/// Scoring subsystem
use super::data::*;
use crate::net::ToDrop;
use crate::protocol::BelongsTo;
use crate::there_is_collision;
use crate::*;
use bevy::color::palettes::tailwind;
use bevy::prelude::*;
use common::board::*;
use lightyear::prelude::ControlledBy;
use lightyear::prelude::server::ClientOf;
use rand::Rng;

/// An event denoting that some lines are cleared
#[derive(EntityEvent)]
#[allow(missing_docs)]
pub struct LinesCleared {
    pub lines_cleared: u32,
    #[event_target]
    pub target: Entity,
}

/// Update the game state when lines are cleared
pub fn update_score(event: On<LinesCleared>, states: Query<(&mut SharedGameState, &ControlledBy)>) {
    let states = build_per_client_table(states);

    for (client, mut state) in states {
        if event.target == client {
            let lines_cleared = event.lines_cleared;
            assert!(lines_cleared <= 4);

            state.lines_cleared += lines_cleared;
            state.lines_cleared_since_last_level += lines_cleared;
            state.score += match lines_cleared {
                0 => 0,
                1 => 40 * (state.level + 1),
                2 => 100 * (state.level + 1),
                3 => 300 * (state.level + 1),
                4 => 1200 * (state.level + 1),
                _ => unreachable!(),
            };

            const LINES_PER_LEVEL: u32 = 10;
            if state.lines_cleared_since_last_level >= LINES_PER_LEVEL * (state.level + 1) {
                state.lines_cleared_since_last_level -= LINES_PER_LEVEL * (state.level + 1);
                state.level += 1;
                let new_gravity = state.drop_interval();
                state.gravity_timer.set_duration(new_gravity);
            }
        }
    }
}

/// Send garbage to (spawn garbage pieces for) an opponent, should also trigger
/// a game over event if this causes the opponent to lose the game
pub fn send_garbage(
    event: On<LinesCleared>,
    priv_states: Query<(&mut PrivateGameState, &ControlledBy)>,
    active_clients: Query<Entity, (With<ClientOf>, Without<ToDrop>)>,
    obstacles: Query<((Entity, &Block), &ControlledBy), With<Obstacle>>,
    active_pcs: Query<((Entity, &Tetromino), &ControlledBy), With<Active>>,
    mut commands: Commands,
) {
    let sender = event.target;
    let lines_cleared = event.lines_cleared;

    if lines_cleared == 0 {
        return;
    }

    let Some(mut priv_state) = take_controlled_by(priv_states, sender) else {
        warn!("send_garbage: No private state for client {sender:?}");
        return;
    };

    if !priv_state.send_garbage {
        return;
    }

    // Collect all active opponents
    let opponents: Vec<Entity> = active_clients.iter().filter(|&e| e != sender).collect();

    if opponents.is_empty() {
        return;
    }

    // Per spec: first pick opponent, then pick hole
    let opponent_idx = priv_state.garbage_rng.random_range(0..opponents.len());
    let opponent = opponents[opponent_idx];

    let hole = priv_state.garbage_rng.random_range(0..BOARD_WIDTH) as i32;

    // Collect opponent's existing obstacles as (Some(entity), block); new garbage as (None, block)
    let mut all_obstacles: Vec<(Option<Entity>, Block)> = filter_controlled_by(obstacles, opponent)
        .map(|(entity, block)| (Some(entity), *block))
        .collect();

    // Get opponent's active tetromino
    let opp_active: Option<(Entity, Tetromino)> = filter_controlled_by(active_pcs, opponent)
        .map(|(entity, tetromino)| (entity, *tetromino))
        .next();

    let mut active_tetromino_opt = opp_active;
    let garbage_color = Color::from(tailwind::GRAY_400);
    let mut game_over = false;

    // Process each garbage line separately per spec
    for _ in 0..lines_cleared {
        // Push all existing obstacles up by one row
        for (_, block) in all_obstacles.iter_mut() {
            block.cell = Cell(block.cell.0, block.cell.1 + 1);
        }

        // Game over if any obstacle leaves the board (including invisible rows)
        if all_obstacles.iter().any(|(_, b)| !b.cell.in_bounds()) {
            game_over = true;
            break;
        }

        // Add new garbage row at y=0 — all columns except the hole
        for x in 0..BOARD_WIDTH as i32 {
            if x != hole {
                all_obstacles.push((
                    None,
                    Block {
                        cell: Cell(x, 0),
                        color: garbage_color,
                    },
                ));
            }
        }

        // Kick active piece up once to resolve any collision
        if let Some((_, ref mut tetromino)) = active_tetromino_opt {
            let blocks: Vec<Block> = all_obstacles.iter().map(|(_, b)| *b).collect();
            if there_is_collision(tetromino, blocks.iter()) {
                tetromino.shift(0, 1);
                if there_is_collision(tetromino, blocks.iter()) {
                    game_over = true;
                    break;
                }
            }
        }
    }

    // Apply obstacle changes: update existing entities or spawn new garbage blocks
    for (entity_opt, block) in &all_obstacles {
        match entity_opt {
            Some(entity) => {
                commands.entity(*entity).insert(*block);
            }
            None => {
                commands.spawn((
                    *block,
                    Obstacle,
                    controlled_by(opponent),
                    BelongsTo(opponent.to_bits()),
                    Replicate::to_clients(NetworkTarget::All),
                ));
            }
        }
    }

    // Persist the kicked tetromino position
    if let Some((entity, tetromino)) = active_tetromino_opt {
        commands.entity(entity).insert(tetromino);
    }

    if game_over {
        commands.trigger(GameOver { entity: opponent });
    }
}
