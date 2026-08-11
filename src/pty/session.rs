use crate::config::TerminalConfig;
use crate::terminal::TerminalGrid;
use anyhow::Context;
use crossbeam_channel::{Receiver, unbounded};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug)]
pub enum PtyCommand {
    Output(Vec<u8>),
    Exited(String),
    Error(String),
}

pub struct PtySession {
    master: Box<dyn MasterPty>,
    child_killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    output_rx: Receiver<PtyCommand>,
}

impl PtySession {
    pub fn spawn(config: TerminalConfig) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.initial_grid.rows,
                cols: config.initial_grid.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open PTY")?;

        let mut command = CommandBuilder::new(&config.shell);
        command.cwd(config.working_dir);
        command.env("TERM", "xterm-256color");

        let mut child = pair.slave.spawn_command(command).context("spawn shell")?;
        let child_killer = Arc::new(Mutex::new(child.clone_killer()));
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("clone PTY reader")?;
        let writer = pair.master.take_writer().context("take PTY writer")?;
        let (output_tx, output_rx) = unbounded();
        let output_tx_for_wait = output_tx.clone();

        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(bytes_read) => {
                        if output_tx
                            .send(PtyCommand::Output(buffer[..bytes_read].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = output_tx.send(PtyCommand::Error(error.to_string()));
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            let status = child.wait().map_err(|error| error.to_string());

            let message = match status {
                Ok(status) => format!("{status:?}"),
                Err(error) => error,
            };

            let _ = output_tx_for_wait.send(PtyCommand::Exited(message));
        });

        Ok(Self {
            master: pair.master,
            child_killer,
            writer: Arc::new(Mutex::new(writer)),
            output_rx,
        })
    }

    pub fn output_rx(&self) -> &Receiver<PtyCommand> {
        &self.output_rx
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer
            .lock()
            .map_err(|_| anyhow::anyhow!("PTY writer lock poisoned"))?
            .write_all(bytes)
            .context("write to PTY")
    }

    pub fn resize(
        &self,
        grid: TerminalGrid,
        cell_width: u16,
        cell_height: u16,
    ) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows: grid.rows,
            cols: grid.cols,
            pixel_width: grid.cols.saturating_mul(cell_width),
            pixel_height: grid.rows.saturating_mul(cell_height),
        })
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if let Ok(mut child_killer) = self.child_killer.lock() {
            let _ = child_killer.kill();
        }
    }
}
