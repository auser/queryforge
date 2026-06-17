use std::path::PathBuf;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> queryforge::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "generate".to_string());
    let config_path = args.next().unwrap_or_else(|| "queryforge.toml".to_string());

    match command.as_str() {
        "generate" => {
            let report =
                queryforge::generate(queryforge::GenerateOptions::from_config_path(config_path))?;
            println!("generated {} queries", report.queries_generated);
            println!("fingerprint {}", report.project_fingerprint);
            for file in report.files_written {
                println!("wrote {}", file.display());
            }
        }
        "check" => {
            let report = queryforge::check(queryforge::CheckOptions {
                config_path: PathBuf::from(config_path),
            })?;
            println!("check {}", if report.ok { "ok" } else { "failed" });
        }
        "prepare" => {
            let report = queryforge::prepare(queryforge::PrepareOptions {
                config_path: PathBuf::from(config_path),
            })?;
            println!("metadata {}", report.metadata_path.display());
        }
        other => {
            eprintln!("unknown command `{other}`");
            eprintln!("usage: queryforge [generate|check|prepare] [queryforge.toml]");
            std::process::exit(2);
        }
    }

    Ok(())
}
