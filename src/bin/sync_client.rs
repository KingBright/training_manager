use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs as tokio_fs;

/// A client to synchronize files from the IsaacLab Manager server.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The address of the server (e.g., http://localhost:8000)
    #[arg(short, long, default_value = "http://127.0.0.1:8000")]
    server: String,

    /// The local directory to sync files into
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Client::builder()
        .timeout(Duration::from_secs(600)) // 10 minute timeout for large files
        .build()?;

    println!("IsaacLab Sync Client");
    println!("--------------------");
    println!("Server: {}", args.server);
    println!("Local Directory: {}", args.dir.display());

    // 1. Fetch server manifest
    println!("\nFetching server file manifest...");
    let manifest_url = format!("{}/api/sync/manifest", args.server);
    let server_manifest = client
        .get(&manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json::<HashMap<String, String>>()
        .await
        .context("Failed to fetch or parse server manifest")?;

    println!("Server has {} files.", server_manifest.len());
    if server_manifest.is_empty() {
        println!("Nothing to sync.");
        return Ok(());
    }

    // 2. Compare and find files to download
    let mut files_to_download = Vec::new();
    println!("\nComparing local files with server manifest...");

    let pb = ProgressBar::new(server_manifest.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")?
            .progress_chars("#>-"),
    );

    for (relative_path, server_hash) in &server_manifest {
        let local_path = args.dir.join(relative_path);
        let mut should_download = true;

        if local_path.exists() {
            if let Some(local_hash) = get_local_hash(&local_path).await? {
                if local_hash == *server_hash {
                    should_download = false;
                }
            }
        }

        if should_download {
            files_to_download.push(relative_path.clone());
        }
        pb.inc(1);
    }
    pb.finish_with_message("Comparison complete.");

    // 3. Download necessary files
    if files_to_download.is_empty() {
        println!("\nAll files are up to date. Nothing to download.");
    } else {
        println!(
            "\nFound {} files to download.",
            files_to_download.len()
        );
        let total_files = files_to_download.len();
        let pb_download = ProgressBar::new(total_files as u64);
        pb_download.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} Downloading [{bar:40.cyan/blue}] {pos}/{len}: {msg}")?
                .progress_chars("=> "),
        );

        for (i, relative_path) in files_to_download.iter().enumerate() {
            pb_download.set_message(relative_path.clone());
            let download_url = format!("{}/api/sync/download/{}", args.server, relative_path);
            let local_path = args.dir.join(relative_path);

            if let Some(parent) = local_path.parent() {
                tokio_fs::create_dir_all(parent).await?;
            }

            let mut response = client.get(&download_url).send().await?.error_for_status()?;
            let mut file = File::create(&local_path)?;

            while let Some(chunk) = response.chunk().await? {
                file.write_all(&chunk)?;
            }

            pb_download.inc(1);
        }
        pb_download.finish_with_message("All files downloaded successfully.");
    }

    println!("\nSync complete!");
    Ok(())
}

/// Calculates the SHA256 hash of a file.
async fn get_local_hash(path: &Path) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let mut file = tokio_fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 1024];

    loop {
        let n = tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    Ok(Some(hash))
}
