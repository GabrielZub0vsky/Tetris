//! Core game logic (server-only, supports multiple clients)

use crate::net::ToDrop;
use crate::score::LinesCleared;
use crate::there_is_collision;
use crate::*;
use bevy::prelude::*;
use common::board::*;
use common::config::GameConfig;
use common::data::*;
use common::protocol::*;
use lightyear::prelude::ServerMultiMessageSender;
use lightyear::prelude::server::*;

/// Handle the input received from the client
pub fn handle_user_input(
    trigger: On<ReceivedInput>,
    states: Query<(&mut SharedGameState, &ControlledBy)>,
    tetrominoes: Query<(&mut Tetromino, &ControlledBy), With<Active>>,
    obstacles: Query<(&Block, &ControlledBy), With<Obstacle>>,
) {
    let client = trigger.target;

    let Some(mut tetromino) = take_controlled_by(tetrominoes, client) else {
        warn!("handle_user_input: Active tetromino is missing for client {client:?}");
        return;
    };

    let Some(state) = take_controlled_by(states, client) else {
        warn!("handle_user_input: Shared game state is missing for client {client:?}");
        return;
    };

    let obstacles: Vec<&Block> = filter_controlled_by(obstacles, client).collect();
    let mut new_tetromino = *tetromino;

    if trigger.inputs.0.contains(&(Input::Left)) {
        new_tetromino.shift(-1, 0);
    }
    if trigger.inputs.0.contains(&(Input::Right)) {
        new_tetromino.shift(1, 0);
    }
    if trigger.inputs.0.contains(&(Input::Down)) {
        for _ in 0..state.manual_drop_gravity {
            new_tetromino.shift(0, -1);
            if there_is_collision(&new_tetromino, obstacles.iter().copied()) {
                new_tetromino.shift(0, 1);
            }
        }
    }
    if trigger.inputs.0.contains(&(Input::Rotate)) {
        new_tetromino.rotate()
    }

    if new_tetromino != *tetromino && !there_is_collision(&new_tetromino, obstacles.iter().copied())
    {
        *tetromino = new_tetromino;
    }
}

/// Drop the piece whenever the gravity timer goes off
///
/// The implementation of this system is given to you as an example.
pub fn gravity(
    states: Query<(&mut SharedGameState, &ControlledBy)>,
    active: Query<(&mut Tetromino, &ControlledBy), With<Active>>,
    obstacles: Query<(&Block, &ControlledBy), With<Obstacle>>,
    time: Res<Time<Fixed>>,
) {
    let mut active = build_per_client_table(active);
    let obstacles = build_per_client_lists(obstacles);
    let states = build_per_client_table(states);

    for (client, mut state) in states {
        let Some(tetromino) = active.get_mut(&client) else {
            warn!("gravity: No tetromino for client {client:?}");
            continue;
        };
        let empty_vec = vec![];
        let obstacles = obstacles.get(&client).unwrap_or(&empty_vec);

        state.gravity_timer.tick(time.delta());
        let mut new_tetromino = *(*tetromino);

        if state.gravity_timer.just_finished() {
            new_tetromino.shift(0, -1);
        }
        if new_tetromino != **tetromino
            && !there_is_collision(&new_tetromino, obstacles.iter().copied())
        {
            **tetromino = new_tetromino;
        }
    }
}

/// Check if the active tetromino cannot move down. If so, deactivate it.
pub fn deactivate_if_stuck(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    lockdowns: Query<(&mut LockdownTimer, &ControlledBy)>,
    active_pcs: Query<((Entity, &mut Tetromino), &ControlledBy), With<Active>>,
    obstacles: Query<(&Block, &ControlledBy), With<Obstacle>>,
) {
    let active = build_per_client_table(active_pcs);
    let obstacles = build_per_client_lists(obstacles);
    let lockdowns = build_per_client_table(lockdowns);

    for (client, mut lockdown) in lockdowns {
        let Some((entity, tetromino)) = active.get(&client) else {
            warn!("deactivate_if_stuck: No tetromino for client {client:?}");
            continue;
        };

        let empty_vec = vec![];
        let obstacles = obstacles.get(&client).unwrap_or(&empty_vec);

        let mut new_tetromino = *(*tetromino);
        new_tetromino.shift(0, -1);

        if there_is_collision(&new_tetromino, obstacles.iter().copied()) {
            lockdown.start_or_advance(&time);

            if lockdown.just_finished() {
                for &cell in tetromino.cells() {
                    commands.spawn((
                        Block {
                            cell,
                            color: tetromino.color,
                        },
                        Obstacle,
                        controlled_by(client),
                        BelongsTo(client.to_bits()),
                        Replicate::to_clients(NetworkTarget::All),
                    ));
                }
                commands.entity(*entity).despawn();
            }
        } else {
            lockdown.reset();
        }
    }
}

/// Spawn the next tetromino if there is no active tetromino.  This should also
/// update the next tetromino window.
pub fn spawn_next_tetromino(
    mut commands: Commands,
    peer_ids: Query<&RemoteId, With<ClientOf>>,
    priv_states: Query<(&mut PrivateGameState, &ControlledBy)>,
    active_pcs: Query<(&Tetromino, &ControlledBy), With<Active>>,
    next_pcs: Query<(&mut Tetromino, &ControlledBy), (With<Next>, Without<Active>)>,
    obstacles: Query<(&Block, &ControlledBy), With<Obstacle>>,
) {
    let active = build_per_client_table(active_pcs);
    let mut next = build_per_client_table(next_pcs);
    let obstacles = build_per_client_lists(obstacles);
    let private_states = build_per_client_table(priv_states);

    for (client, mut priv_state) in private_states {
        if active.contains_key(&client) {
            continue;
        }

        let empty_vec = vec![];
        let obstacles = obstacles.get(&client).unwrap_or(&empty_vec);

        let mut next_piece = priv_state.bag.next_tetromino();
        let offset_x = (BOARD_WIDTH as i32) / 2 - 1;
        let offset_y = (BOARD_HEIGHT as i32) - next_piece.bounds().top - 1;
        next_piece.shift(offset_x, offset_y);

        if !there_is_collision(&next_piece, obstacles.iter().copied()) {
            info!("spawn_next_tetromino: Spawning active piece for client {client:?}");
            commands.spawn((
                next_piece,
                Active,
                controlled_by(client),
                BelongsTo(client.to_bits()),
                Replicate::to_clients(NetworkTarget::All),
            ));
        }

        let mut new_next_piece = priv_state.bag.peek();
        new_next_piece.shift(2, 2);

        if !there_is_collision(&next_piece, obstacles.iter().copied()) {
            match next.get_mut(&client) {
                Some(next_piece) => {
                    **next_piece = new_next_piece;
                }
                None => {
                    // no existing next piece? spawn one & replicate to client
                    commands.spawn((
                        new_next_piece,
                        Next,
                        controlled_by(client),
                        replicate_to(client, &peer_ids),
                    ));
                }
            }
        } else {
            info!("spawn_next_tetromino: Client {client:?} has no room to spawn a new piece");
            commands.trigger(GameOver { entity: client });
        }
    }
}

/// A system to detect the full lines (lines containing only obstacles and no
/// empty space) and to delete them.
///
/// After the lines are deleted, any obstacles above the deleted lines should be
/// moved down using naive gravity (the obstacles move down only by the number
/// of lines below them that are deleted).
pub fn delete_full_lines(
    mut commands: Commands,
    obstacles: Query<((Entity, &mut Block), &ControlledBy), With<Obstacle>>,
) {
    let obstacles = build_per_client_lists(obstacles);

    for (client, obstacles) in obstacles {
        // Count how many obstacle cells exist per row
        let mut cell_count_per_row: [usize; BOARD_HEIGHT as usize] = [0; BOARD_HEIGHT as usize];
        for (_, block) in obstacles.iter() {
            cell_count_per_row[block.cell.1 as usize] += 1;
        }

        // Find the full lines (lines with 10 obstacle cells)
        let full_lines: Vec<usize> = cell_count_per_row
            .iter()
            .enumerate()
            .filter_map(|(row, &count)| {
                if count == BOARD_WIDTH as usize {
                    Some(row)
                } else {
                    None
                }
            })
            .collect();

        if full_lines.is_empty() {
            continue; // No full lines for this client, skip to the next client
        }

        // Create a mapping from old row index to new row index after deletion
        let mut full_lines_below: HashMap<usize, usize> = HashMap::new();
        let mut count = 0;
        for row in 0..BOARD_HEIGHT as usize {
            full_lines_below.insert(row, count);
            if full_lines.contains(&row) {
                count += 1;
            }
        }

        // Delete the obstacles in the full lines and move down the obstacles above them
        for (entity, block) in obstacles.iter() {
            if full_lines.contains(&(block.cell.1 as usize)) {
                commands.entity(*entity).despawn();
            } else {
                let row = block.cell.1 as usize;
                if row < BOARD_HEIGHT as usize && full_lines_below[&row] > 0 {
                    let new_row = row - full_lines_below[&row];
                    commands.entity(*entity).insert(Block {
                        cell: Cell(block.cell.0, new_row as i32),
                        color: block.color,
                    });
                }
            }
        }

        // Update the score and level based on the number of lines cleared
        commands.trigger(LinesCleared {
            lines_cleared: full_lines.len() as u32,
            target: client,
        });
    }
}

/// Swap the current piece and the piece in the hold window on user input.
///
/// If no piece is held, then take the next piece as the active piece and move
/// the current piece to the hold window.
///
/// This system also has to make sure that the swap is legal and kick the piece
/// up by up to 4 times until the swap is legal.  If that is not possible, then
/// abort the swap.
pub fn swap_hold(
    trigger: On<ReceivedInput>,
    mut commands: Commands,
    peer_ids: Query<&RemoteId, With<ClientOf>>,
    priv_states: Query<(&mut PrivateGameState, &ControlledBy)>,
    obstacles: Query<(&Block, &ControlledBy), With<Obstacle>>,
    active_pcs: Query<(&mut Tetromino, &ControlledBy), With<Active>>,
    hold_pcs: Query<(&mut Tetromino, &ControlledBy), (Without<Active>, With<Hold>)>,
    next_pcs: Query<(&mut Tetromino, &ControlledBy), (Without<Active>, Without<Hold>, With<Next>)>,
) {
    let client = trigger.target;

    if !trigger.inputs.0.contains(&Input::Hold) {
        return;
    }

    let Some(mut ps) = take_controlled_by(priv_states, client) else {
        warn!("Private game state is missing for client {client:?}");
        return;
    };
    let Some(mut active) = take_controlled_by(active_pcs, client) else {
        warn!("Active tetromino is missing for client {client:?}");
        return;
    };
    let Some(mut next) = take_controlled_by(next_pcs, client) else {
        warn!("Next tetromino is missing for client {client:?}");
        return;
    };
    let obstacles: Vec<&Block> = filter_controlled_by(obstacles, client).collect();

    // Assigning new hold to current active, calculating shift amt, and shifting
    let active_center = active.center;
    let mut new_hold_piece = *active;
    let new_hold_piece_shamt = (2 - active_center.0 as i32, 2 - active_center.1 as i32);
    new_hold_piece.shift(new_hold_piece_shamt.0, new_hold_piece_shamt.1);

    // New active -> current hold OR next from bag, calculating shift amt, and shifting
    let mut new_active_piece_shamt = (active_center.0 as i32, active_center.1 as i32);

    let hold_piece_op = take_controlled_by(hold_pcs, client);

    let mut new_active_piece = if let Some(hold) = hold_piece_op.as_ref() {
        new_active_piece_shamt.0 -= 2;
        new_active_piece_shamt.1 -= 2;
        **hold
    } else {
        ps.bag.next_tetromino()
    };
    new_active_piece.shift(new_active_piece_shamt.0, new_active_piece_shamt.1);

    // checking for collision and shifting up to 4 times if there is a collision
    let mut is_collision = true;
    for _ in 0..4 {
        if there_is_collision(&new_active_piece, obstacles.iter().copied()) {
            new_active_piece.shift(0, 1);
        } else {
            is_collision = false;
            break;
        }
    }

    // assuming no collision:
    // if hold piece exists, then swapping active and hold pieces
    // if hold piece does not exist, then spawning new hold piece in hold window, updating active piece, and updating next piece window
    if !is_collision {
        if let Some(mut hold) = hold_piece_op {
            *hold = new_hold_piece;
            *active = new_active_piece;
        } else {
            commands.spawn((
                new_hold_piece,
                Hold,
                controlled_by(client),
                replicate_to(client, &peer_ids),
            ));
            *active = new_active_piece;

            let mut new_next_piece = ps.bag.peek();
            new_next_piece.shift(2, 2);
            *next = new_next_piece;
        }
    }
}

/// Update hard drop state based on user input
pub fn update_hard_drop(
    trigger: On<ReceivedInput>,
    states: Query<(&mut SharedGameState, &ControlledBy)>,
) {
    let client = trigger.target;

    let Some(mut state) = take_controlled_by(states, client) else {
        warn!("Shared game state is missing for client {client:?}");
        return;
    };

    if trigger.inputs.0.contains(&Input::HardDrop) {
        state.hard_drop = !state.hard_drop;
        state.manual_drop_gravity = if state.hard_drop {
            HARD_DROP_GRAVITY
        } else {
            SOFT_DROP_GRAVITY
        };
    }
}

/// Notify the player if they have won (i.e., if they are the last player
/// remaining), only on multiplayer games.
///
/// Instead of immediately disconnecting the winner, this sends `WonContinue`
/// so the client can prompt the user to either stop or keep playing solo.
/// The winner stays connected and active until they respond via
/// `handle_continue_choice`.
pub fn check_winner(
    cfg: Res<GameConfig>,
    newly_dropped: Query<Entity, (With<ClientOf>, Added<ToDrop>)>,
    active_clients: Query<Entity, (With<ClientOf>, Without<ToDrop>)>,
    mut outcomes: ResMut<crate::GameOutcomes>,
    mut awaiting: ResMut<crate::AwaitingContinue>,
    server: Single<&Server>,
    mut sender: ServerMultiMessageSender,
    peer_ids: Query<&RemoteId, With<ClientOf>>,
) {
    if cfg.expected_players <= 1 || newly_dropped.is_empty() {
        return;
    }

    let remaining: Vec<Entity> = active_clients.iter().collect();
    if remaining.len() != 1 {
        return;
    }

    let winner = remaining[0];
    if awaiting.winner == Some(winner) {
        return;
    }

    outcomes.outcomes.insert(winner, "Won".to_string());
    awaiting.winner = Some(winner);

    let Ok(peer) = peer_ids.get(winner) else {
        return;
    };
    info!("sending WonContinue prompt to {winner}");
    sender
        .send::<GameOverMessage, StateChange>(
            &GameOverMessage::WonContinue,
            &server,
            &NetworkTarget::Single(peer.0),
        )
        .expect("Could not send the WonContinue message!");
}

/// React to the winner's continue-or-stop choice after WonContinue.
///
/// * ContinueNo  → trigger the normal GameOver flow (writes stats on exit,
///   disconnects the client, tears the lobby down).
/// * ContinueYes → write the stats immediately, mark the game finalized so
///   the OnExit handler doesn't double-write, and let the player keep
///   playing until they naturally lose (no more stats updates after this).
pub fn handle_continue_choice(
    trigger: On<ReceivedInput>,
    mut commands: Commands,
    mut awaiting: ResMut<crate::AwaitingContinue>,
    mut tracking: ResMut<crate::GameTracking>,
    outcomes: Res<crate::GameOutcomes>,
    client_order: Res<crate::ClientOrder>,
    db_cfg: Option<Res<crate::ServerDbConfig>>,
) {
    let client = trigger.target;
    if awaiting.winner != Some(client) {
        return;
    }

    let yes = trigger.inputs.0.contains(&Input::ContinueYes);
    let no = trigger.inputs.0.contains(&Input::ContinueNo);
    if !yes && !no {
        return;
    }

    if no {
        info!("winner {client} chose to stop — triggering GameOver");
        awaiting.winner = None;
        commands.trigger(GameOver { entity: client });
        return;
    }

    info!("winner {client} chose to keep playing — finalizing stats now");
    if let Some(db_cfg) = db_cfg.as_deref() {
        crate::record::write_game_result_impl(db_cfg, &client_order, &outcomes, &tracking);
    }
    tracking.finalized = true;
    awaiting.winner = None;
}

/// Queue the client to be disconnected for when the GameOver event is triggered.
///
/// Sends Won or Lost based on any verdict already recorded in GameOutcomes (defaults to Lost).
pub fn disconnect_on_game_over(
    game_over: On<GameOver>,
    server: Single<&Server>,
    mut sender: ServerMultiMessageSender,
    peer_ids: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
    state: Res<State<ServerState>>,
    mut outcomes: ResMut<crate::GameOutcomes>,
) {
    if *state.get() != ServerState::Running {
        return;
    }

    let verdict = outcomes
        .outcomes
        .entry(game_over.entity)
        .or_insert_with(|| "Lost".to_string())
        .clone();
    let msg = if verdict == "Won" {
        GameOverMessage::Won
    } else {
        GameOverMessage::Lost
    };

    info!("sending game over ({verdict}) to {}", game_over.entity);
    sender
        .send::<GameOverMessage, StateChange>(
            &msg,
            &server,
            &NetworkTarget::Single(peer_ids.get(game_over.entity).unwrap().0),
        )
        .expect("Could not send the message!");
    commands.entity(game_over.entity).insert(ToDrop);
}
