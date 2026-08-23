#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod audit;
mod backend;
mod design;
mod ui;
mod updater;

use std::{borrow::Cow, sync::Arc};

use gpui::{Application, AssetSource, SharedString};

use backend::Backend;

struct CedarAssets;

impl AssetSource for CedarAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == "cedar/app-icon.png" {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/128x128.png"))));
        }

        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut assets = gpui_component_assets::Assets.list(path)?;
        if "cedar/app-icon.png".starts_with(path) {
            assets.push("cedar/app-icon.png".into());
        }
        Ok(assets)
    }
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .iter()
        .any(|argument| argument == "--version" || argument == "-V")
    {
        println!("Cedar {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|argument| argument == "--list-visual-qa") {
        println!("{}", ui::VISUAL_QA_SCENARIOS.join("\n"));
        return;
    }
    let visual_qa = match ui::VisualQaConfig::from_args(&args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("invalid visual QA options: {error:#}");
            std::process::exit(2);
        }
    };
    let backend = Arc::new(
        if visual_qa.is_some() {
            Backend::new_visual_qa()
        } else {
            Backend::new()
        }
        .expect("failed to initialize Cedar's local backend"),
    );
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("cedar-worker")
            .build()
            .expect("failed to initialize Cedar's async runtime"),
    );

    Application::new().with_assets(CedarAssets).run(move |cx| {
        gpui_component::init(cx);
        design::init(cx).expect("failed to initialize Cedar's design system");
        let result = if let Some(config) = visual_qa {
            ui::open_visual_qa_window(cx, backend.clone(), runtime.clone(), config)
        } else {
            ui::open_main_window(cx, backend.clone(), runtime.clone())
        };
        match result {
            Ok(()) => {
                if visual_qa.is_none()
                    && let Err(error) = updater::complete_update_health()
                {
                    eprintln!("failed to report a healthy Cedar update: {error:#}");
                }
            }
            Err(error) => {
                eprintln!("failed to open Cedar window: {error:#}");
                cx.quit();
            }
        }
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_cedar_icon_is_embedded_as_a_png_asset() {
        let icon = CedarAssets
            .load("cedar/app-icon.png")
            .expect("icon asset should load")
            .expect("icon asset should exist");

        assert_eq!(&icon[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(icon.as_ref(), include_bytes!("../assets/128x128.png"));
    }
}
