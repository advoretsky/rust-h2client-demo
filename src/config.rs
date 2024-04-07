use std::env;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Config {
    pub(crate) base_url: Option<String>,
    base_dir: Option<PathBuf>,
    pub(crate) files_number: Option<u32>,
    pub(crate) concurrency: u32,
    pub(crate) max_streams: u32,
    pub(crate) insecure: bool,
}

pub(crate) fn parse_args() -> Config {
    let args: Vec<String> = env::args().collect();

    let mut config = Config {
        base_url: None,
        base_dir: None,
        files_number: None,
        concurrency: 1,
        max_streams: 500,
        insecure: false,
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
                if let Some(value) = args.get(i).and_then(|val| val.parse().ok()) {
                    config.concurrency = value
                }
            }
            "-s" | "--max-streams" => {
                i += 1;
                if let Some(value) = args.get(i).and_then(|val| val.parse().ok()) {
                    config.max_streams = value
                }
            }
            "-k" | "--insecure" => {
                config.insecure = true
            }

            _ => {
                eprintln!("Unknown option: {}", args[i]);
            }
        }
        i += 1;
    }

    config
}