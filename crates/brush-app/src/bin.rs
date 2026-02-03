#![recursion_limit = "256"]

mod shared;

use brush_cli::Cli;
use brush_process::create_process;
use brush_ui::app::App;
use clap::Parser;
use rand;
use std::env;
use std::fs;
use std::path::PathBuf;
use underfolder_to_nerf::convert_underfolder_to_nerf;

use crate::shared::startup;

#[cfg(target_family = "windows")]
fn is_console() -> bool {
    let mut buffer = [0u32; 1];

    // SAFETY: FFI, buffer is large enough.
    unsafe {
        use winapi::um::wincon::GetConsoleProcessList;
        let count = GetConsoleProcessList(buffer.as_mut_ptr(), 1);
        count != 1
    }
}

#[allow(clippy::unnecessary_wraps)] // Error isn't need on wasm but that's ok.
fn main() -> Result<(), anyhow::Error> {
    let args = Cli::parse().validate()?;

    startup();

    #[cfg(target_family = "windows")]
    if args.with_viewer && !is_console() {
        // Hide the console window on windows when running as a GUI.
        // SAFETY: FFI.
        unsafe {
            winapi::um::wincon::FreeConsole();
        };
    }

    #[cfg(feature = "tracy")]
    {
        use tracing_subscriber::layer::SubscriberExt;

        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(tracing_tracy::TracyLayer::default()),
        )
        .expect("Failed to set tracing subscriber");
    }

    // Convert source to nerf
    let temp_path = if let Some(source) = &args.source {
        let p = match source {
            brush_vfs::DataSource::Path(p) => p,
            _ => panic!("Only local paths can be converted to nerf datasets"),
        };
        let temp_dir = env::temp_dir();
        let random_id = rand::random::<u64>();
        let out_path_str = temp_dir
            .join(format!("brush_{}", random_id))
            .to_string_lossy()
            .to_string();
        let converted_path = convert_underfolder_to_nerf(
            p,
            &out_path_str,
            &args.train_stream.load_config.image_key,
            &args.train_stream.load_config.mask_key,
            &args.train_stream.load_config.camera_key,
            &args.train_stream.load_config.w2c_key,
        );
        Some(PathBuf::from(converted_path))
    } else {
        None
    };

    let converted_source = temp_path
        .as_ref()
        .map(|p| brush_vfs::DataSource::Path(p.to_string_lossy().to_string()));

    let temp_path_export_base = env::temp_dir();
    let temp_path_export = temp_path_export_base
        .join(format!("brush_out_{}", rand::random::<u64>()))
        .to_string_lossy()
        .to_string();
    let temp_ply_name = "brush_out.ply";
    let temp_path_export_ply = PathBuf::from(&temp_path_export).join(&temp_ply_name);

    let mut args = args;
    args.train_stream.process_config.export_path = temp_path_export.clone();
    args.train_stream.process_config.export_name = temp_ply_name.to_string();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize tokio runtime")
        .block_on(async move {
            // Create initial process if source is provided
            let init_process = converted_source.map(|source| {
                create_process(
                    source,
                    #[cfg(feature = "training")]
                    {
                        let cli_config = args.train_stream.clone();
                        async move |init| {
                            brush_process::args_file::merge_configs(&init, &cli_config)
                        }
                    },
                )
            });

            if args.with_viewer {
                env_logger::builder()
                    .target(env_logger::Target::Stdout)
                    .init();

                let icon = eframe::icon_data::from_png_bytes(
                    &include_bytes!("../assets/icon-256.png")[..],
                )
                .expect("Failed to load icon");

                let native_options = eframe::NativeOptions {
                    // Build app display.
                    viewport: egui::ViewportBuilder::default()
                        .with_inner_size(egui::Vec2::new(1450.0, 1200.0))
                        .with_active(true)
                        .with_icon(std::sync::Arc::new(icon)),
                    wgpu_options: brush_ui::create_egui_options(),
                    persist_window: true,
                    ..Default::default()
                };

                let title = if cfg!(debug_assertions) {
                    "Brush  -  Debug"
                } else {
                    "Brush"
                };

                // UI will init the burn device.
                eframe::run_native(
                    title,
                    native_options,
                    Box::new(move |cc| Ok(Box::new(App::new(cc, init_process)))),
                )?;
            } else {
                // Manually init the device.
                brush_process::burn_init_setup().await;
                brush_cli::run_cli_ui(
                    init_process.expect("Must provide a source"),
                    args.train_stream,
                )
                .await?;
            }

            anyhow::Result::<(), anyhow::Error>::Ok(())
        })?;

    // Convert the ply to an Eyesplat Neural Twin file.
    // Calling the python script defined in ./python/decode_splat.py
    if let Some(out_nt) = &args.out_nt {
        let ply_path = temp_path_export_ply;
        let output_path = match out_nt {
            brush_vfs::DataSource::Path(p) => p,
            _ => panic!("Only local paths can be used for output Eyesplat Neural Twin"),
        };
        let status = std::process::Command::new("python3")
            .arg("python/decode_splat.py")
            .arg(ply_path)
            .arg(output_path)
            .status()
            .expect("Failed to execute decode_splat.py");
        if !status.success() {
            panic!("decode_splat.py failed with status: {}", status);
        }
    }

    // Clean up temp directories
    if let Some(path) = temp_path {
        fs::remove_dir_all(&path).ok();
    };
    fs::remove_dir_all(&temp_path_export_base).ok();

    Ok(())
}
