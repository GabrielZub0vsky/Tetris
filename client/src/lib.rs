//! Client support library.

// these two flags are allowed because these warnings are triggered by many
// systems.
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]

use bevy::app::App;
use common::*;

pub mod board;
pub mod ui;

use common::data::Hold;

/// Load given path via a web request.
pub async fn get_web_resource(path: &str) -> Result<String, wasm_bindgen::JsValue> {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestInit, RequestMode, Response};

    let opts = RequestInit::new();
    opts.set_method("GET");
    opts.set_mode(RequestMode::Cors);

    let request = Request::new_with_str_and_init(path, &opts)?;

    let window = web_sys::window().unwrap();
    let resp_value = JsFuture::from(window.fetch_with_request(&request)).await?;

    assert!(resp_value.is_instance_of::<Response>());
    let resp: Response = resp_value.dyn_into().unwrap();
    let value = JsFuture::from(resp.text()?).await?;

    web_sys::console::log_1(&value);
    Ok(value.as_string().unwrap())
}

/// Inject the systems and plugins for this game into the app.
///
/// This is the client version, so it should include only the input and
/// rendering logic.
pub fn build_app(app: &mut App, cfg: config::GameConfig) {
    use bevy::prelude::*;
    use board::*;
    use data::Next;
    use ui::*;
    app
        // .insert_resource(cfg.build_game_state())
        .add_systems(Startup, (setup_board, setup_ui).chain().in_set(Game))
        // TODO: handle game over
        .add_systems(
            Update,
            (
                handle_user_input,
                redraw_board,
                redraw_side_board::<Next>,
                redraw_side_board::<Hold>,
                receive_game_over,
            )
                .in_set(Game),
        );

    // #[cfg(all(not(feature = "ci"), not(feature = "test")))]
    app.add_systems(Startup, setup_window);

    if cfg.animate_title {
        app.add_systems(Update, animate_title);
    }

    // UI subsystems
    app.add_systems(
        Update,
        (animate_time, update_score_text, update_hard_drop_text),
    );
}
