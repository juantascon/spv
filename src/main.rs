use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::fs;
use std::io;
use std::path::PathBuf;

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
    Ls,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { id, cmd, args } => {
            let id = id.unwrap_or_else(|| cmd.clone());
            let pid = PID::from_id(id.clone());
            pid.write()?;
            let mut child = supervisor::exec(cmd, args)?;
            supervisor::supervise(id.clone(), &mut child).await?;
            pid.delete()?
        }

        Commands::Stop { id } => {
            let pid = PID::from_id(id);
            pid.signal(Some(Signal::SIGTERM))?
        }

        Commands::Restart { id } => {
            let pid = PID::from_id(id);
            pid.signal(Some(Signal::SIGUSR1))?
        }

        Commands::Ls => {
            for pid in PID::ls() {
                if pid.is_alive() {
                    println!("{}", pid.id);
                }
            }
        }
    }

    Ok(())
}

mod cfg {
    use std::path::PathBuf;

    pub fn run_dir() -> PathBuf {
        match std::env::var("SPV_RUNTIME_DIR") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => {
                let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
                PathBuf::from(base).join("spv")
            }
        }
    }
}

struct PID {
    id: String,
    dir: PathBuf,
    pid_path: PathBuf,
}

impl PID {
    pub fn ls() -> Vec<PID> {
        let Ok(entries) = fs::read_dir(cfg::run_dir()) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| {
                let id = entry.ok()?.file_name().into_string().ok()?;
                Some(PID::from_id(id))
            })
            .collect()
    }

    pub fn from_id(id: String) -> Self {
        let dir = cfg::run_dir().join(&id);
        let pid_path = dir.join("pid");
        Self {
            id: id,
            dir: dir,
            pid_path: pid_path,
        }
    }

    pub fn write(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(&self.pid_path, std::process::id().to_string())
    }

    pub fn delete(&self) -> io::Result<()> {
        fs::remove_dir_all(&self.dir)
    }

    pub fn read(&self) -> Result<Pid> {
        let pid: i32 = fs::read_to_string(&self.pid_path)
            .context(format!("process not found: {:?}", self.id))?
            .trim()
            .parse()
            .context("invalid pid read from file")?;
        Ok(Pid::from_raw(pid))
    }

    pub fn is_alive(&self) -> bool {
        self.signal(None).is_ok()
    }

    pub fn signal(&self, sig: Option<Signal>) -> Result<()> {
        let pid = self.read()?;
        signal::kill(pid, sig).context("unable to send signal")
    }
}

mod supervisor {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;
    use std::process::Stdio;
    use tokio::io::Result;
    use tokio::process::{Child, Command};
    use tokio::signal::unix::{SignalKind, signal as tokio_signal};

    pub fn exec(cmd: String, args: Vec<String>) -> Result<Child> {
        Command::new(cmd.clone())
            .args(args.clone())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
    }

    pub async fn supervise(id: String, child: &mut Child) -> Result<()> {
        let mut sigusr1 = tokio_signal(SignalKind::user_defined1())?;
        let mut sigterm = tokio_signal(SignalKind::terminate())?;

        loop {
            println!("[spv]: supervising {}", id);

            tokio::select! {
                _ = child.wait() => {
                    println!("\n[spv] {} exited, restarting ...", id);
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
                    break;
                }
            }
        }
        Ok(())
    }
}
