#![allow(missing_docs)]

use bevy::{color::palettes::tailwind, prelude::*};
use common::data::SharedGameState;

pub const ELEMENT_OUTLINE: Color = Color::Srgba(tailwind::GRAY_400);
pub const ELEMENT_FILL: Color = Color::Srgba(tailwind::PINK_500);
pub const TEXT_SIZE: f32 = 20.0;
pub const BG_COLOR: Color = Color::BLACK;
pub const PADDING: f32 = 30.0;

#[derive(Component)]
pub struct TitleText;

#[derive(Component)]
pub struct TimeText;

#[derive(Component)]
pub struct ScoreText;

#[derive(Component, Default)]
pub struct HardDropText;

pub fn mk_text(text: impl Into<String>, rest: impl Bundle, font_size: Option<f32>) -> impl Bundle {
    (
        Text(text.into()),
        TextFont {
            font_size: font_size.unwrap_or(TEXT_SIZE),
            ..Default::default()
        },
        rest,
    )
}

pub fn setup_ui(mut commands: Commands) {
    commands.spawn(mk_text(
        "BLOCK DROPPER 3000!",
        (
            TextColor(Color::from(tailwind::PINK_300)),
            Node {
                margin: auto().horizontal(),
                top: px(PADDING / 3.0),
                ..default()
            },
            TitleText,
            UiTransform::from_translation(Val2::px(0.0, 10.0)),
        ),
        Some(TEXT_SIZE + 10.0),
    ));

    commands.spawn((mk_text(
        r"Help:
left, right: Move
down, space: Drop
z:           Enable/disable
             hard drop
x:           Swap hold
",
        Node {
            top: percent(10),
            left: percent(5),
            ..default()
        },
        Some(10.0),
    ),));

    commands.spawn((mk_text(
        r"Time:",
        (
            Node {
                top: percent(95),
                left: percent(5),
                ..default()
            },
            TimeText,
        ),
        Some(20.0),
    ),));

    // Initialize the score text
    commands.spawn((
        ScoreText,
        mk_text(
            "Score: 0\nLevel: 0",
            Node {
                top: percent(85),
                left: percent(5),
                ..default()
            },
            None,
        ),
    ));

    // Initialize hard drop text
    commands.spawn(mk_text(
        "Hard Drop: Off",
        (
            Node {
                width: percent(40),
                height: percent(100),
                row_gap: px(10),
                ..default()
            },
            HardDropText,
            UiTransform::from_translation(Val2::percent(10.0, 80.0)),
        ),
        None,
    ));
}

pub fn animate_title(mut title: Single<&mut UiTransform, With<TitleText>>, time: Res<Time>) {
    title.rotation = Rot2::degrees((time.elapsed_secs() * 2.0).sin() * 10.0);
}

pub fn animate_time(mut time_t: Single<&mut Text, With<TimeText>>, time: Res<Time<Fixed>>) {
    time_t.0 = format!("Time: {:?}", time.elapsed());
}

/// Update the score text.
pub fn update_score_text(
    state: Single<&SharedGameState>,
    mut score: Single<&mut Text, With<ScoreText>>,
) {
    info!(
        "in score. Score: {}\nLevel: {}",
        state.score(),
        state.level()
    );
    score.0 = format!("Score: {}\nLevel: {}", state.score(), state.level());
}

pub fn update_hard_drop_text(
    state: Single<&SharedGameState>,
    mut hard_drop: Single<&mut Text, With<HardDropText>>,
) {
    hard_drop.0 = if state.hard_drop {
        "Hard Drop: On".to_string()
    } else {
        "Hard Drop: Off".to_string()
    };
}
