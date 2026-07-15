#![recursion_limit = "256"]

// The desktop binary only compiles on native platforms.
// On WASM, brush-app is used as a library (cdylib) via wasm.rs instead.
#[cfg(not(target_family = "wasm"))]
mod ui;

#[cfg(not(target_family = "wasm"))]
#[allow(clippy::unnecessary_wraps)]
fn main() -> Result<(), anyhow::Error> {
    use brush_cli::Cli;
    use brush_process::DataSource;
    use clap::Parser;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use underfolder_to_nerf::convert_underfolder_to_nerf;

    let args = Cli::parse().validate()?;

    #[cfg(target_family = "windows")]
    {
        use winapi::um::wincon::GetConsoleProcessList;

        let mut buffer = [0u32; 1];

        // Safety: FFI. Buffer is valid for duration of call
        let is_console = unsafe { GetConsoleProcessList(buffer.as_mut_ptr(), 1) != 1 };

        if args.with_viewer && !is_console {
            // Safety: FFI
            unsafe {
                winapi::um::wincon::FreeConsole();
            };
        }
    }

    #[cfg(feature = "tracy")]
    {
        use tracing_subscriber::layer::SubscriberExt;

        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(tracing_tracy::TracyLayer::default()),
        )
        .expect("Failed to set tracing subscriber");
    }

    // Convert source to nerf if needed
    let temp_path = if let Some(source) = args.get_source() {
        let p = match source {
            DataSource::Path(p) => p,
            _ => panic!("Only local paths can be converted to nerf datasets"),
        };
        let temp_dir = env::temp_dir();
        let random_id = rand::random::<u64>();
        let out_path_str = temp_dir
            .join(format!("brush_{}", random_id))
            .to_string_lossy()
            .to_string();
        let converted_path = convert_underfolder_to_nerf(
            &p,
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
        .map(|p| DataSource::Path(p.to_string_lossy().to_string()));

    // Setup temporary export paths for PLY if we need to convert to .nt
    let (temp_path_export, temp_path_export_ply) = if args.output_file.is_some() {
        let temp_path_export_base = env::temp_dir();
        let temp_path_export = temp_path_export_base
            .join(format!("brush_out_{}", rand::random::<u64>()));
        fs::create_dir_all(&temp_path_export)?;
        let temp_ply_name = "brush_out.ply";
        let temp_path_export_ply = temp_path_export.join(temp_ply_name);
        (Some(temp_path_export), Some(temp_path_export_ply))
    } else {
        (None, None)
    };

    let mut args = args;
    
    // Override export paths if we're doing .nt conversion
    if let (Some(export_dir), Some(_)) = (&temp_path_export, &temp_path_export_ply) {
        args.train_stream.process_config.export_path = export_dir.to_string_lossy().to_string();
        args.train_stream.process_config.export_name = "brush_out.ply".to_string();
    }

    let output_file = args.output_file.clone();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize tokio runtime")
        .block_on(async move {
            // Use converted source if available, otherwise use original source
            let effective_source = converted_source.or_else(|| args.get_source());
            
            let init_process = effective_source.map(|source| {
                let cli_config = args.train_stream.clone();
                brush_process::create_process(source, async move |init| {
                    Some(brush_process::args_file::merge_configs(&init, &cli_config))
                })
            });

            if args.with_viewer {
                use crate::ui::app::App;

                let logger = env_logger::Builder::from_default_env()
                    .target(env_logger::Target::Stdout)
                    .build();
                let max = logger.filter();
                crate::ui::log_panel::install_global_logger(Box::new(logger), max);

                let icon = eframe::icon_data::from_png_bytes(
                    &include_bytes!("../assets/icon-256.png")[..],
                )
                .expect("Failed to load icon");

                let native_options = eframe::NativeOptions {
                    viewport: egui::ViewportBuilder::default()
                        .with_inner_size(egui::Vec2::new(1450.0, 1200.0))
                        .with_active(true)
                        .with_icon(std::sync::Arc::new(icon)),
                    wgpu_options: ui::create_egui_options(),
                    persist_window: true,
                    ..Default::default()
                };

                let title = if cfg!(debug_assertions) {
                    "Brush  -  Debug"
                } else {
                    "Brush"
                };

                eframe::run_native(
                    title,
                    native_options,
                    Box::new(move |cc| Ok(Box::new(App::new(cc, init_process)))),
                )?;
            } else {
                let process = init_process.expect("Must provide a source");
                brush_cli::run_headless(process, args.train_stream).await?;
            }

            anyhow::Result::<(), anyhow::Error>::Ok(())
        })?;

    // Convert the ply to an Eyesplat Neural Twin file if output_file was specified
    if let (Some(output_path), Some(ply_path)) = (output_file, temp_path_export_ply) {
        eprintln!("Converting PLY to Neural Twin format...");
        let status = std::process::Command::new("python3")
            .arg("python/decode_splat.py")
            .arg(&ply_path)
            .arg(&output_path)
            .status()
            .expect("Failed to execute decode_splat.py");
        if !status.success() {
            eprintln!("Warning: decode_splat.py failed with status: {}", status);
        } else {
            eprintln!("Successfully saved to: {}", output_path);
        }
    }

    // Clean up temp directories
    if let Some(path) = temp_path {
        fs::remove_dir_all(&path).ok();
    }
    if let Some(path) = temp_path_export {
        fs::remove_dir_all(&path).ok();
    }

    Ok(())
}

// On WASM, just stub a dummy main.
#[cfg(target_family = "wasm")]
fn main() {}
