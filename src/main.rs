use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::fs;
use std::io;
use std::path::PathBuf;
use tokio::runtime::Runtime;

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

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { id, cmd, args } => {
            let id = id.unwrap_or_else(|| cmd.clone());
            let pid = PID::from_id(id.clone());
            let rt = Runtime::new()?;
            rt.block_on(async move {
                pid.write()?;
                supervisor::supervise(id.clone(), cmd, args).await?;
                pid.delete()?;
                Ok::<(), anyhow::Error>(())
            })?;
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
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;
    use tokio::signal::unix::{SignalKind, signal as tokio_signal};
    use tokio::time::{Duration, sleep};

    pub async fn supervise(id: String, cmd: String, args: Vec<String>) -> Result<()> {
        let mut first_iteration = true;
        const SLEEP_TIME: Duration = Duration::from_millis(500);
        'outer: loop {
            if !first_iteration {
                sleep(SLEEP_TIME).await;
            }
            first_iteration = true;

            let Ok(mut child) = Command::new(cmd.clone())
                .args(args.clone())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
            else {
                println!("[spv:{}]: spawn() error, restarting ...", id);
                continue;
            };

            let Some(pid) = child.id() else {
                println!("[spv:{}]: pawn() didn't return pid, restarting ...", id);
                continue;
            };

            let stdout = child.stdout.take().expect("failed to open stdout");
            let stderr = child.stderr.take().expect("failed to open stderr");
            let mut stdout_lines = BufReader::new(stdout).lines();
            let mut stderr_lines = BufReader::new(stderr).lines();

            let mut sigusr1 = tokio_signal(SignalKind::user_defined1())?;
            let mut sigterm = tokio_signal(SignalKind::terminate())?;

            println!("[spv:{}]: supervisor start, child pid: {:?}", id, pid);

            loop {
                tokio::select! {
                    Ok(Some(line)) = stdout_lines.next_line() => {
                        println!("[{}]: {}", id, line);
                    }
                    Ok(Some(line)) = stderr_lines.next_line() => {
                        println!("[{}]: {}", id, line);
                    }
                    _ = child.wait() => {
                        println!("[spv:{}] child exited, restarting ...", id);
                        break;
                    }
                    _ = sigusr1.recv() => {
                        println!("[spv:{}] received SIGUSR1, restarting ...", id);
                        signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM).ok();
                        break;
                    }
                    _ = sigterm.recv() => {
                        println!("[spv:{}] received SIGTERM, terminating child ...", id);
                        signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM).ok();
                        break 'outer;
                    }
                }
            }
        }
        println!("[spv:{}]: supervisor end", id);
        Ok(())
    }
}
