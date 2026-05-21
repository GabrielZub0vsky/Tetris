//! The tetris board setup

use std::collections::BTreeSet;
use std::collections::HashSet;

use bevy::color::palettes::tailwind;
use lightyear::prelude::Client;
use lightyear::prelude::Connected;
use lightyear::prelude::MessageReceiver;
use lightyear::prelude::MessageSender;

use crate::ui::TitleText;
use crate::ui::{BG_COLOR, PADDING};
use common::protocol::*;

use bevy::{platform::collections::HashMap, prelude::*};
use common::board::*;
use common::data::*;

// Create a logical tile to insert into a board.
//
// Board width and board height are the information of the board this tile is
// placed in.
fn mk_tile(
    x: u32,
    y: u32,
    board_width: u32,
    board_height: u32,
    tile_mesh: Handle<Mesh>,
    tile_material: Handle<ColorMaterial>,
) -> impl Bundle {
    (
        Mesh2d(tile_mesh),
        MeshMaterial2d(tile_material),
        (
            Text2d(format!("{x},{y}")),
            TextFont {
                font_size: 12.0,
                ..Default::default()
            },
        ),
        Transform::from_xyz(
            (x as f32 - board_width as f32 / 2.0) * TILE_SIDE_LEN + PADDING / 2.0,
            (y as f32 - board_height as f32 / 2.0) * TILE_SIDE_LEN + PADDING / 2.0,
            0.0,
        )
        .with_scale(Vec3::splat(0.8)),
    )
}

/// The calculated window height based on the board size.
pub const fn window_height() -> f32 {
    TILE_SIDE_LEN * BOARD_HEIGHT as f32 + PADDING * 2.0 + crate::ui::TEXT_SIZE * 2.0
}

/// The calculated window width based on the board size.
pub const fn window_width() -> f32 {
    window_height()
}

/// Set up a side window to show the next piece, the hold area, or an opponent's board
pub fn spawn_side_window(
    transform: Transform,
    mesh: Handle<Mesh>,
    material: Handle<ColorMaterial>,
    commands: &mut Commands,
    title: &str,
    marker: impl Component + Copy,
    (width, height): (u32, u32),
) {
    commands
        .spawn((transform, Visibility::default()))
        .with_children(|parent| {
            (0..height).for_each(|y| {
                (0..width).for_each(|x| {
                    parent.spawn((
                        mk_tile(x, y, width, height, mesh.clone(), material.clone()),
                        Block {
                            cell: Cell(x as i32, y as i32),
                            color: BG_COLOR,
                        },
                        marker,
                    ));
                })
            });

            parent.spawn((
                Transform::from_xyz(
                    -((width - 1) as f32) * TILE_SIDE_LEN * 0.5,
                    height as f32 * TILE_SIDE_LEN * 0.5,
                    -1.0,
                ),
                Text2d::new(title),
            ));
        });
}

/// Set up the window. Only used when not testing.
pub fn setup_window(mut window: Single<&mut Window>) {
    window.resolution.set(window_height(), window_height());
}

/// Create the board and initialize game data
pub fn setup_board(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    // Background color
    commands.insert_resource(ClearColor(Color::BLACK));

    // Set up the camera
    commands.spawn(Camera2d);

    let mesh = meshes.add(Rectangle::new(TILE_SIDE_LEN, TILE_SIDE_LEN));
    let material = materials.add(BG_COLOR);

    // Set up the board
    let _cells = (0..BOARD_HEIGHT)
        .map(|y| {
            (0..BOARD_WIDTH)
                .map(|x| {
                    commands
                        .spawn((
                            mk_tile(
                                x,
                                y,
                                BOARD_WIDTH,
                                BOARD_HEIGHT,
                                mesh.clone(),
                                material.clone(),
                            ),
                            Block {
                                cell: Cell(x as i32, y as i32),
                                color: BG_COLOR,
                            },
                            GameBoard::MainBoard,
                        ))
                        .id()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    // next window
    spawn_side_window(
        Transform::from_xyz(
            (BOARD_WIDTH + 5) as f32 * TILE_SIDE_LEN * 0.5 + PADDING,
            window_height() * 0.5 - 5.0 * TILE_SIDE_LEN * 0.5 - PADDING,
            0.0,
        )
        .with_scale(Vec3::splat(0.8)),
        mesh.clone(),
        material.clone(),
        &mut commands,
        "Next",
        Next,
        (5, 5),
    );

    // hold window
    crate::board::spawn_side_window(
        Transform::from_xyz(
            (BOARD_WIDTH + 5) as f32 * TILE_SIDE_LEN * 0.5 + PADDING,
            -window_height() * 0.5 + 5.0 * TILE_SIDE_LEN * 0.5 + PADDING,
            0.0,
        )
        .with_scale(Vec3::splat(0.8)),
        mesh.clone(),
        material.clone(),
        &mut commands,
        "Hold",
        Hold,
        (5, 5),
    );

    // The left window
    spawn_side_window(
        Transform::from_xyz(
            -((BOARD_WIDTH + 5) as f32) * TILE_SIDE_LEN * 0.5 - PADDING,
            window_height() * 0.5 - 5.0 * TILE_SIDE_LEN * 0.5 - PADDING - 300.,
            0.0,
        )
        .with_scale(Vec3::splat(0.5)),
        mesh.clone(),
        material.clone(),
        &mut commands,
        "Left",
        GameBoard::Left,
        (10, 20),
    );

    // The right window
    spawn_side_window(
        Transform::from_xyz(
            (BOARD_WIDTH + 5) as f32 * TILE_SIDE_LEN * 0.5 + PADDING,
            window_height() * 0.5 - 5.0 * TILE_SIDE_LEN * 0.5 - PADDING - 300.,
            0.0,
        )
        .with_scale(Vec3::splat(0.5)),
        mesh.clone(),
        material.clone(),
        &mut commands,
        "Right",
        GameBoard::Right,
        (10, 20),
    );
}

/// Read the keyboard input and send it to the server as a message.
pub fn handle_user_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut sender: Single<&mut MessageSender<Inputs>, (With<Client>, With<Connected>)>,
) {
    let mut inputs = Inputs(HashSet::new());

    if keyboard.just_pressed(KeyCode::ArrowDown) {
        inputs.0.insert(Input::Down);
    }
    if keyboard.just_pressed(KeyCode::ArrowLeft) {
        inputs.0.insert(Input::Left);
    }
    if keyboard.just_pressed(KeyCode::ArrowRight) {
        inputs.0.insert(Input::Right);
    }
    if keyboard.any_just_pressed([KeyCode::ArrowUp, KeyCode::Space]) {
        inputs.0.insert(Input::Rotate);
    }
    if keyboard.just_pressed(KeyCode::KeyX) {
        inputs.0.insert(Input::Hold);
    }
    if keyboard.just_pressed(KeyCode::KeyZ) {
        inputs.0.insert(Input::HardDrop);
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        inputs.0.insert(Input::EndGame);
    }

    if !inputs.0.is_empty() {
        info!(
            "client handle_user_input: Sending user inputs: {:?}",
            inputs
        );
        sender.send::<InputChannel>(inputs);
    }
}

/// Redraw the boards for the current player and each opponent.
pub fn redraw_board(
    mut commands: Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
    tetrominoes: Query<(&Tetromino, &BelongsTo), With<Active>>,
    obstacles: Query<(&Block, &BelongsTo), With<Obstacle>>,
    board: Query<(&Block, Entity, &GameBoard)>,
    state: Option<Single<&SharedGameState>>,
) {
    let Some(state) = state else { return };
    let my_id = state.client_id;

    // Collect sorted opponent IDs to identify left (min) and right (max) boards.
    let mut opponent_ids = BTreeSet::new();
    for (_, b) in tetrominoes.iter() {
        if b.0 != my_id {
            opponent_ids.insert(b.0);
        }
    }
    for (_, b) in obstacles.iter() {
        if b.0 != my_id {
            opponent_ids.insert(b.0);
        }
    }
    let left_id = opponent_ids.iter().next().copied();
    let right_id = opponent_ids.iter().next_back().copied();

    let mut main_cells = HashMap::<Cell, Color>::new();
    let mut left_cells = HashMap::<Cell, Color>::new();
    let mut right_cells = HashMap::<Cell, Color>::new();

    let mut insert_cell_color = |belongs_to: u64, my_id, cell: Cell, color: Color| {
        let hashmap = if belongs_to == my_id {
            &mut main_cells
        } else if Some(belongs_to) == left_id {
            &mut left_cells
        } else if Some(belongs_to) == right_id {
            &mut right_cells
        } else {
            unreachable!()
        };

        hashmap.insert(cell, color);
    };

    for (block, belongs_to) in obstacles.iter() {
        if !block.cell.is_visible() {
            continue;
        }
        insert_cell_color(belongs_to.0, my_id, block.cell, block.color);
    }

    for (tet, belongs_to) in tetrominoes.iter() {
        for &cell in tet.cells() {
            if cell.is_visible() {
                insert_cell_color(belongs_to.0, my_id, cell, tet.color);
            }
        }
    }

    for (block, entity, game_board) in board.iter() {
        let color = match game_board {
            GameBoard::MainBoard => main_cells.get(&block.cell),
            GameBoard::Left => left_cells.get(&block.cell),
            GameBoard::Right => right_cells.get(&block.cell),
        };
        commands
            .entity(entity)
            .insert(MeshMaterial2d(materials.add(*color.unwrap_or(&BG_COLOR))));
    }
}

/// Redraw the side board with the given marker component.
pub fn redraw_side_board<Marker: Component>(
    mut commands: Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut side_board: Query<(&Block, Entity), With<Marker>>,
    tetromino: Option<Single<&Tetromino, With<Marker>>>,
) {
    let mut cells = HashMap::<Cell, Color>::new();

    if let Some(t) = tetromino {
        // For each cell of the preview tetromino, find the matching side tile and color it.
        for &cell in t.cells() {
            cells.insert(cell, t.color);
        }
    }

    for (block, entity) in side_board.iter_mut() {
        let color = cells.get(&(block.cell));
        commands
            .entity(entity)
            .insert(MeshMaterial2d(materials.add(*color.unwrap_or(&BG_COLOR))));
    }
}

/// Receive the game over message from the server and update the UI.
pub fn receive_game_over(
    mut receiver: Single<&mut MessageReceiver<GameOverMessage>>,
    mut title: Single<(&mut Text, &mut TextColor), With<TitleText>>,
) {
    let messages = receiver
        .receive_with_tick()
        .map(|message| (message.data, message.remote_tick))
        .collect::<Vec<_>>();
    if !messages.is_empty() {
        info!("Received: {messages:?}");
        for msg in messages {
            if matches!(msg.0, GameOverMessage::Won) {
                title.0.0 = "You won!".to_string();
                title.1.0 = Color::from(tailwind::GREEN_400);
            } else {
                title.0.0 = "You lost!".to_string();
                title.1.0 = Color::from(tailwind::RED_400);
            }
        }
    }
}
