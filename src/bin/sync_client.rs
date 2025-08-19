use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs as tokio_fs;

use zip::ZipArchive;

/// A client to synchronize and download files from the IsaacLab Manager server.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The address of the server (e.g., http://localhost:3000)
    #[arg(short, long, global = true, default_value = "http://127.0.0.1:3000")]
    server: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Synchronize a directory from the server (downloading only changed files)
    Sync {
        /// The local directory to sync files into
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// The remote directory on the server to sync from
        #[arg(long)]
        remote_dir: Option<String>,
    },
    /// Download and extract a directory from the server as a ZIP archive
    Download {
        /// The remote directory on the server to download
        #[arg(long)]
        remote_path: String,

        /// The local directory to extract files into
        #[arg(short, long, default_value = ".")]
        local_path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(600)) // 10 minute timeout for large files
        .build()?;

    println!("IsaacLab Client");
    println!("---------------");
    println!("Server: {}", args.server);

    match args.command {
        Commands::Sync { dir, remote_dir } => handle_sync(&client, &args.server, &dir, remote_dir.as_ref()).await?,
        Commands::Download { remote_path, local_path } => handle_download(&client, &args.server, &remote_path, &local_path).await?,
    }

    Ok(())
}

async fn handle_download(client: &Client, server: &str, remote_path: &str, local_path: &Path) -> Result<()> {
    println!("\nDownloading directory '{}'...", remote_path);
    println!("Target local path: {}", local_path.display());

    let url = format!("{}/api/sync/download_zip", server);
    let mut response = client
        .get(url)
        .query(&[("remote_path", remote_path)])
        .send()
        .await?
        .error_for_status()?;

    let total_size = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{msg}\n{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
        .progress_chars("=> "));
    pb.set_message(format!("Downloading {}", remote_path));

    let mut temp_file = tempfile::tempfile()?;
    let mut downloaded: u64 = 0;

    while let Some(chunk) = response.chunk().await? {
        temp_file.write_all(&chunk)?;
        downloaded = std::cmp::min(downloaded + chunk.len() as u64, total_size);
        pb.set_position(downloaded);
    }
    pb.finish_with_message("Download complete.");

    println!("\nExtracting archive...");

    // The extraction process is synchronous, so we run it in a blocking task
    let local_path_buf = local_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        temp_file.seek(std::io::SeekFrom::Start(0))?;
        let mut archive = ZipArchive::new(temp_file)?;

        std::fs::create_dir_all(&local_path_buf)?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let outpath = match file.enclosed_name() {
                Some(path) => local_path_buf.join(path),
                None => continue,
            };

            if file.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(&p)?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }
        Ok(())
    }).await??;

    println!("Extraction complete. Files are in {}", local_path.display());
    Ok(())
}

async fn handle_sync(client: &Client, server: &str, dir: &Path, remote_dir: Option<&String>) -> Result<()> {
    if let Some(remote_dir) = remote_dir {
        println!("Remote Directory: {}", remote_dir);
    }
    println!("Local Directory: {}", dir.display());

    // 1. Fetch server manifest
    println!("\nFetching server file manifest...");
    let manifest_url = format!("{}/api/sync/manifest", server);
    let mut request = client.get(&manifest_url);
    if let Some(rd) = remote_dir {
        request = request.query(&[("remote_path", rd)]);
    }
    let server_manifest = request
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
        let local_path = dir.join(relative_path);
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

        for (_i, relative_path) in files_to_download.iter().enumerate() {
            pb_download.set_message(relative_path.clone());
            let download_url = format!("{}/api/sync/download/{}", server, relative_path);

            let mut request = client.get(&download_url);
            if let Some(rd) = remote_dir {
                request = request.query(&[("remote_path", rd)]);
            }

            let local_path = dir.join(relative_path);

            if let Some(parent) = local_path.parent() {
                tokio_fs::create_dir_all(parent).await?;
            }

            let mut response = request.send().await?.error_for_status()?;
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
