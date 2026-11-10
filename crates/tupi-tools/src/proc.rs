use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tupi_kernel::ToolError;

/// Teto de bytes guardados por stream. O excedente e drenado e descartado
/// para o processo nao travar com o pipe cheio.
const CAPTURE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub timed_out: bool,
    pub duration: Duration,
}

pub fn cargo(args: &[String], cwd: &Path, timeout: Duration) -> Result<Output, ToolError> {
    let mut command = Command::new("cargo");
    command
        .args(args)
        .current_dir(cwd)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        .env("NO_COLOR", "1");
    run(command, timeout)
}

pub fn bash(command: &str, cwd: &Path, timeout: Duration) -> Result<Output, ToolError> {
    let mut cmd = Command::new("bash");
    cmd.arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .env("NO_COLOR", "1");
    run(cmd, timeout)
}

pub fn program(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<Output, ToolError> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env("NO_COLOR", "1");
    run(command, timeout)
}

fn run(mut command: Command, timeout: Duration) -> Result<Output, ToolError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate(&mut command);

    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| ToolError::Other(format!("falha ao iniciar processo: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Other("stdout indisponivel".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::Other("stderr indisponivel".to_string()))?;
    let out_reader = std::thread::spawn(move || read_capped(stdout));
    let err_reader = std::thread::spawn(move || read_capped(stderr));

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                kill_tree(&mut child);
                timed_out = true;
                break child.wait().map_err(|error| {
                    ToolError::Other(format!("falha ao encerrar processo: {error}"))
                })?;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                return Err(ToolError::Other(format!(
                    "falha ao acompanhar processo: {error}"
                )));
            }
        }
    };

    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(Output {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        success: status.success() && !timed_out,
        timed_out,
        duration: started.elapsed(),
    })
}

#[cfg(unix)]
fn isolate(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn isolate(_command: &mut Command) {}

#[cfg(unix)]
fn kill_tree(child: &mut std::process::Child) {
    let pgid = child.id() as i32;
    // SAFETY: o filho foi criado com process_group(0), entao o pid e o pgid.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn read_capped(mut pipe: impl Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < CAPTURE_LIMIT {
                    let room = CAPTURE_LIMIT - kept.len();
                    kept.extend_from_slice(&buffer[..n.min(room)]);
                }
            }
            Err(_) => break,
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_returns_stdout_and_success() {
        let out = bash("printf oi", Path::new("."), Duration::from_secs(10)).unwrap();
        assert!(out.success);
        assert!(out.stdout.contains("oi"));
        assert!(!out.timed_out);
    }

    #[test]
    fn a_command_that_exceeds_the_timeout_is_killed() {
        let out = bash("sleep 5", Path::new("."), Duration::from_millis(150)).unwrap();
        assert!(out.timed_out);
        assert!(!out.success);
        assert!(out.duration < Duration::from_secs(3));
    }

    #[test]
    fn a_failing_command_is_not_success() {
        let out = bash("exit 3", Path::new("."), Duration::from_secs(10)).unwrap();
        assert!(!out.success);
        assert!(!out.timed_out);
    }
}
