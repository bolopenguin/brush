use colored::*;
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use serde_json;
use serde_yaml;
use std::fs;
use std::io;
use walkdir::WalkDir;

use image::{ImageReader, RgbaImage};

#[derive(Debug, Clone)]
struct CameraParams {
    width: u32,
    height: u32,
    camera_matrix: [[f32; 3]; 3],
    distortion_coeffs: [f32; 5],
}

#[derive(Debug, Clone)]
struct FrameParams {
    file_path: String,
    transform_matrix: [[f32; 4]; 4],
    camera_angle_x: f32,
    camera_angle_y: f32,
    fl_x: f32,
    fl_y: f32,
    k1: f32,
    k2: f32,
    p1: f32,
    p2: f32,
    cx: f32,
    cy: f32,
    w: u32,
    h: u32,
}

/// Finds all files in the given path that contain the specified substring in their filename.
fn find_files_with_sub_str(path: &str, sub_str: &str) -> io::Result<Vec<String>> {
    let mut results = Vec::new();
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        let file_name = entry.file_name().to_string_lossy();
        if file_name.contains(sub_str) {
            results.push(entry.path().to_string_lossy().to_string());
        }
    }
    Ok(results)
}

fn parse_camera_file(path: &str) -> io::Result<CameraParams> {
    // Assert it is a yaml or json file
    let is_yaml = path.ends_with(".yaml") || path.ends_with(".yml");
    let is_json = path.ends_with(".json");
    assert!(
        is_yaml || is_json,
        "Camera file must be .yaml, .yml, or .json"
    );

    let content = fs::read_to_string(path)?;
    let d: serde_json::Value = if is_json {
        serde_json::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
    } else {
        // Parse YAML and convert to JSON Value for uniform handling
        let yaml_val: serde_yaml::Value = serde_yaml::from_str(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        serde_json::to_value(yaml_val).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
    };

    // Parse the YAML data
    let intrinsics = d.get("intrinsics").ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "Missing 'intrinsics' section")
    })?;

    // Parse image size
    let image_size = intrinsics
        .get("image_size")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing or invalid 'image_size'",
            )
        })?;
    if image_size.len() != 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "image_size must have exactly 2 elements",
        ));
    }
    let width = image_size[0]
        .as_u64()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid width"))?
        as u32;
    let height = image_size[1]
        .as_u64()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid height"))?
        as u32;

    // Parse camera matrix
    let camera_matrix_yaml = intrinsics
        .get("camera_matrix")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing or invalid 'camera_matrix'",
            )
        })?;
    if camera_matrix_yaml.len() != 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "camera_matrix must have exactly 3 rows",
        ));
    }

    let mut camera_matrix = [[0.0f32; 3]; 3];
    for (i, row) in camera_matrix_yaml.iter().enumerate() {
        let row_seq = row.as_array().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("camera_matrix row {} is not a sequence", i),
            )
        })?;
        if row_seq.len() != 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("camera_matrix row {} must have exactly 3 elements", i),
            ));
        }
        for (j, val) in row_seq.iter().enumerate() {
            camera_matrix[i][j] = val.as_f64().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Invalid camera_matrix value at [{},{}]", i, j),
                )
            })? as f32;
        }
    }

    // Parse distortion coefficients
    let dist_coeffs_yaml = intrinsics
        .get("dist_coeffs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing or invalid 'dist_coeffs'",
            )
        })?;

    let mut distortion_coeffs = [0.0f32; 5];
    for (i, val) in dist_coeffs_yaml.iter().enumerate() {
        if i >= 5 {
            break; // Only take first 5 coefficients
        }
        distortion_coeffs[i] = val.as_f64().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid dist_coeffs value at index {}", i),
            )
        })? as f32;
    }

    Ok(CameraParams {
        width,
        height,
        camera_matrix,
        distortion_coeffs,
    })
}

fn parse_w2c_file(path: &str) -> io::Result<nalgebra::Matrix4<f32>> {
    assert!(path.ends_with(".txt"));

    let content = fs::read_to_string(path)?;

    let values: Vec<f32> = content
        .split_whitespace()
        .map(|s| s.parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    if values.len() != 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Expected 16 values, got {}", values.len()),
        ));
    }

    Ok(nalgebra::Matrix4::from_row_slice(&values))
}

fn parse_image(path_image: &str, path_mask: &str) -> io::Result<RgbaImage> {
    // Load image and mask once, directly to desired format
    let mut img = ImageReader::open(path_image)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .decode()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .to_rgba8();

    let mask = ImageReader::open(path_mask)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .decode()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        .to_luma8();

    // Ensure mask dimensions match image
    if img.dimensions() != mask.dimensions() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Image and mask dimensions do not match",
        ));
    }

    // Directly modify alpha channel in place - much faster than parallel overhead
    for (pixel, &mask_value) in img.pixels_mut().zip(mask.pixels()) {
        pixel[3] = mask_value[0];
    }

    Ok(img)
}

fn rotate_x_axis(matrix: &nalgebra::Matrix4<f32>) -> nalgebra::Matrix4<f32> {
    // Rotation of 180 degrees around x axis
    let mut rx = nalgebra::Matrix4::<f32>::identity();
    rx[(1, 1)] = -1.0;
    rx[(2, 2)] = -1.0;
    matrix * rx
}

/// Converts an underfolder dataset to NERF format.
pub fn convert_underfolder_to_nerf(
    path_in: &str,
    path_out: &str,
    image_key: &str,
    mask_key: &str,
    camera_key: &str,
    w2c_key: &str,
) -> String {
    println!(
        "Converting underfolder '{}' to nerf dataset '{}'...",
        path_in.green().bold(),
        path_out.green().bold()
    );
    println!(
        "Using keys: image='{}', mask='{}', camera='{}', w2c='{}'",
        image_key.blue().bold(),
        mask_key.blue().bold(),
        camera_key.blue().bold(),
        w2c_key.blue().bold()
    );

    let mut image_files = find_files_with_sub_str(path_in, image_key).unwrap();
    image_files.sort();
    assert!(!image_files.is_empty(), "No image files found");
    let mut mask_files = find_files_with_sub_str(path_in, mask_key).unwrap();
    mask_files.sort();
    assert!(!mask_files.is_empty(), "No mask files found");
    let mut camera_files = find_files_with_sub_str(path_in, camera_key).unwrap();
    camera_files.sort();
    assert!(!camera_files.is_empty(), "No camera files found");
    let mut w2c_files = find_files_with_sub_str(path_in, w2c_key).unwrap();
    w2c_files.sort();
    assert!(!w2c_files.is_empty(), "No w2c files found");

    assert!(image_files.len() == mask_files.len());
    assert!(image_files.len() == w2c_files.len());
    if camera_files.len() != 1 {
        assert!(
            image_files.len() == camera_files.len(),
            "Number of camera files must be 1 or equal to number of image files"
        );
    }

    let mut cameras: Vec<CameraParams> = Vec::new();
    for camera_file in camera_files.iter() {
        let camera_params = parse_camera_file(camera_file).unwrap();
        cameras.push(camera_params);
    }
    if cameras.len() == 1 {
        for _ in 1..image_files.len() {
            cameras.push(cameras[0].clone());
        }
    }

    // Create progress bar for frame processing

    // Create progress bar
    let pb = ProgressBar::new(image_files.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );

    // Parallel processing with progress
    let frames: Vec<FrameParams> = (0..image_files.len())
        .into_par_iter()
        .progress_with(pb)
        .map(|i| {
            let camera = &cameras[i];
            let dist_coeffs = camera.distortion_coeffs;
            let w = camera.width;
            let h = camera.height;
            let fl_x = camera.camera_matrix[0][0];
            let fl_y = camera.camera_matrix[1][1];
            let cx = camera.camera_matrix[0][2];
            let cy = camera.camera_matrix[1][2];
            let angle_x = 2.0 * (w as f32 / (2.0 * fl_x)).atan();
            let angle_y = 2.0 * (h as f32 / (2.0 * fl_y)).atan();

            // Parse image and mask
            let image_file = &image_files[i];
            let mask_file = &mask_files[i];
            let image = parse_image(image_file, mask_file).unwrap();

            // Ensure output folder exists (safe to call in parallel)
            fs::create_dir_all(format!("{}/images", path_out)).unwrap();

            let inner_path = format!("images/{}_image.png", i);
            let output_image_path = format!("{}/{}", path_out, inner_path);
            image.save(&output_image_path).unwrap();

            // Load w2c file and apply rotation
            let w2c_file = &w2c_files[i];
            let w2c = parse_w2c_file(w2c_file).unwrap();
            let c2w = rotate_x_axis(&w2c);
            // Convert to row-major format
            let c2w: [[f32; 4]; 4] = [
                [c2w[(0, 0)], c2w[(0, 1)], c2w[(0, 2)], c2w[(0, 3)]],
                [c2w[(1, 0)], c2w[(1, 1)], c2w[(1, 2)], c2w[(1, 3)]],
                [c2w[(2, 0)], c2w[(2, 1)], c2w[(2, 2)], c2w[(2, 3)]],
                [c2w[(3, 0)], c2w[(3, 1)], c2w[(3, 2)], c2w[(3, 3)]],
            ];

            FrameParams {
                file_path: inner_path,
                transform_matrix: c2w,
                camera_angle_x: angle_x,
                camera_angle_y: angle_y,
                fl_x,
                fl_y,
                k1: dist_coeffs[0],
                k2: dist_coeffs[1],
                p1: dist_coeffs[2],
                p2: dist_coeffs[3],
                cx,
                cy,
                w,
                h,
            }
        })
        .collect();

    // Save the frames into a JSON file under path_out/transforms.json
    let json_data = serde_json::json!({
        "frames": frames.iter().map(|f| {
            serde_json::json!({
                "file_path": f.file_path,
                "transform_matrix": f.transform_matrix,
                "camera_angle_x": f.camera_angle_x,
                "camera_angle_y": f.camera_angle_y,
                "fl_x": f.fl_x,
                "fl_y": f.fl_y,
                "k1": f.k1,
                "k2": f.k2,
                "p1": f.p1,
                "p2": f.p2,
                "cx": f.cx,
                "cy": f.cy,
                "w": f.w,
                "h": f.h,
            })
        }).collect::<Vec<_>>(),
    });

    let json_path = format!("{}/transforms.json", path_out);
    fs::write(
        &json_path,
        serde_json::to_string_pretty(&json_data).unwrap(),
    )
    .expect("Failed to write transforms.json");

    println!("Conversion result: {}", path_out.green().bold());
    path_out.to_string()
}
