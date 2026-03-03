use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::signal::unix::{SignalKind, signal as tokio_signal};

#[derive(Parser)]
#[command(name = "spv", version = "1.0", about = "Simple Process Supervisor")]

struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Start {
        #[arg(short, long, help = "if not present, defaults to cmd")]
        id: Option<String>,
        cmd: String,
        args: Vec<String>,
    },
    Stop {
        id: String,
    },
    Restart {
        id: String,
    },
    Logs {
        id: String,
    },
    Ls,
    #[command(hide = true)]
    Supervise {
        id: String,
        cmd: String,
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { id, cmd, args } => {
            Command::new(std::env::current_exe()?)
                .arg("supervise")
                .arg("--id")
                .arg(id.unwrap_or_else(|| cmd.clone()))
                .arg(&cmd)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
        }

        Commands::Supervise { id, cmd, args } => {
            let spv = SPV::from_id(id);
            spv.prepare()?;
            spv.pid_write()?;
            let mut child = spv.spawn(cmd, args)?;
            spv.supervise(&mut child).await?
        }

        Commands::Stop { id } => {
            let spv = SPV::from_id(id);
            signal::kill(spv.pid_read()?, Signal::SIGTERM)?;
        }

        Commands::Restart { id } => {
            let spv = SPV::from_id(id);
            signal::kill(spv.pid_read()?, Signal::SIGUSR1)?;
        }

        Commands::Logs { id } => {
            let spv = SPV::from_id(id);
            println!("{}", spv.log_read()?);
        }

        Commands::Ls => {
            for spv in SPV::ls() {
                if spv.is_alive() {
                    println!("{}", spv.id);
                }
            }
        }
    }

    Ok(())
}

struct SPV {
    id: String,
    dir: PathBuf,
    pid_path: PathBuf,
    log_path: PathBuf,
}

impl SPV {
    pub fn ls() -> Vec<SPV> {
        let Ok(entries) = fs::read_dir(SPV::run_dir()) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| {
                let id = entry.ok()?.file_name().into_string().ok()?;
                Some(SPV::from_id(id))
            })
            .collect()
    }

    fn run_dir() -> PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(base).join("spv")
    }

    pub fn from_id(id: String) -> Self {
        let dir = SPV::run_dir().join(&id);
        let pid_path = dir.join("pid");
        let log_path = dir.join("log");
        Self {
            id: id,
            dir: dir,
            pid_path: pid_path,
            log_path: log_path,
        }
    }

    pub fn prepare(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)
    }

    pub fn pid_write(&self) -> io::Result<()> {
        fs::write(&self.pid_path, std::process::id().to_string())
    }

    pub fn pid_delete(&self) -> io::Result<()> {
        fs::remove_file(&self.pid_path)
    }

    pub fn pid_read(&self) -> Result<Pid> {
        let pid: i32 = fs::read_to_string(&self.pid_path)
            .context(format!("process not found: {:?}", self.id))?
            .trim()
            .parse()
            .context("invalid pid")?;
        Ok(Pid::from_raw(pid))
    }

    pub fn log_file(&self) -> Result<File> {
        File::options()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .context(format!("failed to open log file: {:?}", self.log_path))
    }

    pub fn log_read(&self) -> Result<String> {
        fs::read_to_string(&self.log_path).context(format!("process not found: {:?}", self.id))
    }

    pub fn is_alive(&self) -> bool {
        let Ok(pid) = self.pid_read() else {
            return false;
        };
        signal::kill(pid, None).is_ok()
    }

    pub fn spawn(&self, cmd: String, args: Vec<String>) -> Result<Child> {
        Ok(Command::new(cmd.clone())
            .args(args.clone())
            .stdout(self.log_file()?)
            .stderr(self.log_file()?)
            .kill_on_drop(true)
            .spawn()?)
    }

    pub async fn supervise(&self, child: &mut Child) -> Result<()> {
        let mut sigusr1 = tokio_signal(SignalKind::user_defined1())?;
        let mut sigterm = tokio_signal(SignalKind::terminate())?;

        loop {
            println!("[spv]: supervising {}", self.id);

            tokio::select! {
                _ = child.wait() => {
                    println!("\n[spv] {} exited, restarting ...", self.id);
                    continue;
                }
                _ = sigusr1.recv() => {
                    if let Some(pid) = child.id() {
                        signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM).ok();
                    }
                    continue;
                }
                _ = sigterm.recv() => {
                    if let Some(pid) = child.id() {
                        signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM).ok();
                    }
                    self.pid_delete().ok();
                    break;
                }
            }
        }
        Ok(())
    }
}
