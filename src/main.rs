use anyhow::Result;
use futures::prelude::*;
use rustyline::{DefaultEditor, ExternalPrinter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use ya_service_bus::RpcMessage;

#[derive(Serialize, Deserialize)]
struct ChatMsg {
    msg: String,
}

impl RpcMessage for ChatMsg {
    const ID: &'static str = "ChatMsg";
    type Item = ();
    type Error = ();
}

enum Command {
    Send { to: String, msg: String },
}

fn interaction() -> Result<(impl ExternalPrinter, mpsc::Receiver<Command>)> {
    let (mut tx, rx) = mpsc::channel(1);
    let mut rl = DefaultEditor::new()?;
    let printer = rl.create_external_printer()?;

    fn interactive_loop(
        mut rl: DefaultEditor,
        tx: &mut mpsc::Sender<Command>,
    ) -> anyhow::Result<()> {
        let mut ctx: Option<String> = None;
        let mut prompt = "=> ".to_owned();
        loop {
            let line = rl.readline(&prompt)?;
            if line.is_empty() {
                continue;
            }
            rl.add_history_entry(&line)?;
            if let Some(line) = line.strip_prefix('=') {
                let ctx_ref = line.trim();
                ctx = Some(ctx_ref.to_owned());
                prompt = format!("[{ctx_ref}] => ");
                continue;
            }
            if let Some(to) = ctx.clone() {
                let msg = line;
                tx.blocking_send(Command::Send { to, msg })?;
            } else {
                eprintln!("setup context, for eg. =0x889ff52ece3d5368051f4f8216650a7843f8926b");
            }
        }
    }

    std::thread::spawn(move || {
        if let Err(e) = interactive_loop(rl, &mut tx) {
            eprintln!("Error: {:?}", e);
        }
        drop(tx)
    });

    Ok((printer, rx))
}

#[actix_rt::main]
async fn main() -> Result<()> {
    use ya_service_bus::typed as bus;
    let (printer, mut it) = interaction()?;
    let printer = Arc::new(tokio::sync::RwLock::new(printer));

    let _ = {
        let printer = printer.clone();

        bus::bind_with_caller("/public/chat", move |caller: String, msg: ChatMsg| {
            let printer = printer.clone();
            async move {
                let r = printer
                    .write()
                    .await
                    .print(format!("[from: {caller}]: {}", msg.msg));
                if let Err(e) = r {
                    eprintln!("callback failed: {:?}", e);
                }
                Ok(())
            }
        });
    };

    while let Some(cmd) = it.recv().await {
        let printer = printer.clone();
        match cmd {
            Command::Send { to, msg } => {
                let f = async move {
                    let r = bus::service(format!("/net/{to}/chat"))
                        .call(ChatMsg { msg })
                        .await;
                    if let Err(e) = r {
                        printer
                            .write()
                            .await
                            .print(format!("send error: {:?}", e))?;
                    } else {
                        printer.write().await.print("send done!".into())?;
                    }
                    Ok(())
                };

                actix_rt::spawn(
                    f.map_err(|e: anyhow::Error| eprintln!("failed to print smg: {:?}", e)),
                );
            }
        }
    }

    Ok(())
}
