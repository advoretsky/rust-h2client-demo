use std::{env, io};
use futures::stream::StreamExt;

use tokio::sync::Semaphore;
use std::sync::Arc;
use std::path::PathBuf;
use std::process::ExitCode;
use reqwest::ClientBuilder;
use sysinfo::{CpuRefreshKind, Pid};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

#[derive(Debug)]
enum MyError {
    Io(io::Error),
    Reqwest(reqwest::Error),
}

impl From<io::Error> for MyError {
    fn from(err: io::Error) -> MyError {
        MyError::Io(err)
    }
}

impl From<reqwest::Error> for MyError {
    fn from(err: reqwest::Error) -> MyError {
        MyError::Reqwest(err)
    }
}

async fn fetch_with_limit(
    base_url: String,
    paths: Vec<String>,
    base_dir: PathBuf,
    limit: u32
) -> Result<(), MyError> {
    let client = ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let semaphore = Arc::new(Semaphore::new(limit as usize));

    let tasks: Vec<_> = paths.iter().map(|path| {
        let url = format!("{}{}", base_url, path);
        let file_path = base_dir.join(path);
        let client = client.clone();
        let permit = semaphore.clone().acquire_owned();

        async move {
            let _permit = permit.await.unwrap();
            let response = client.get(&url).send().await?;
            let mut file = File::create(&file_path).await?;
            let mut stream = response.bytes_stream();

            while let Some(item) = stream.next().await {
                let chunk = item?;
                file.write_all(&chunk).await?;
            }

            Ok::<(), MyError>(())
        }
    }).collect();

    for task in futures::future::join_all(tasks).await {
        // Since task is already Result<(), MyError>, we can use `?` directly
        task?;
    }

    Ok(())
}

#[derive(Debug)]
struct Config {
    base_url: Option<String>,
    base_dir: Option<PathBuf>,
    files_number: Option<u32>,
    concurrency: Option<u32>
}

fn parse_args() -> Config {
    let args: Vec<String> = env::args().collect();

    let mut config = Config {
        base_url: None,
        base_dir: None,
        files_number: None,
        concurrency: None,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-u" | "--base-url" => {
                i += 1;
                config.base_url = args.get(i).cloned();
            }
            "-d" | "--base-dir" => {
                i += 1;
                config.base_dir = args.get(i).map(PathBuf::from);
            }
            "-n" | "--files-number" => {
                i += 1;
                config.files_number = args.get(i).and_then(|val| val.parse().ok());
            }
            "-c" | "--concurrency" => {
                i += 1;
                config.concurrency = args.get(i).and_then(|val| val.parse().ok());
            }
            _ => {
                // Report unknown options
                eprintln!("Unknown option: {}", args[i]);
            }
        }
        i += 1;
    }

    config
}

#[tokio::main]
async fn main() -> Result<(), ExitCode>{
    let config = parse_args();

    let mut required_params_missing = false;
    // Check for missing required options
    if config.base_url.is_none() {
        required_params_missing = true;
        eprintln!("Missing required option: -u/--base-url");
    }
    if config.base_dir.is_none() {
        required_params_missing = true;
        eprintln!("Missing required option: -d/--base-dir");
    }
    if config.files_number.is_none() {
        required_params_missing= true;
        eprintln!("Missing required option: -n/--files-number");
    }

    if required_params_missing {
        return Err(ExitCode::FAILURE)
    }

    let paths : Vec<String> = (1..=config.files_number.unwrap()).map(|i| format!("file-{:06}.json", i)).collect();

    let start_time = Instant::now();

    match fetch_with_limit(config.base_url.unwrap(), paths, config.base_dir.unwrap(), config.concurrency.unwrap_or(10)).await {
        Ok(()) => println!("Files fetched successfully"),
        Err(e) => eprintln!("Error fetching files: {:?}", e),
    }

    // Measure the time taken by the main function
    let end_time = Instant::now();
    let elapsed_time = end_time - start_time;
    println!("Total time: {:?}", elapsed_time);

    // Get CPU usage for the current process
    let mut system = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::new().with_cpu(
            CpuRefreshKind::everything()
        )
    );
    system.refresh_all();
    let process = system.process(Pid::from(std::process::id() as usize)).unwrap();
    let cpu_usage = process.cpu_usage();
    println!("CPU Usage: {:.2}%", cpu_usage);

    Ok(())
}