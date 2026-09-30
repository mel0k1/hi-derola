use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::chat::{Message, Role};
use crate::mcp::McpClient;
use crate::provider::{ApiEvent, ChatRequest, Provider};
use crate::tools;

const MAX_ROUNDS: usize = 15;

pub async fn run(
    provider: Arc<dyn Provider>,
    mut req: ChatRequest,
    tx: UnboundedSender<ApiEvent>,
    allow_all: Arc<AtomicBool>,
    mcp: Option<Arc<McpClient>>,
) -> Result<()> {
    let mut specs = tools::specs();
    if let Some(m) = &mcp {
        specs.extend(m.specs().await);
    }
    req.tools = specs;
    let mut msgs = req.messages.clone();
    let mut round = 0;
    loop {
        round += 1;
        if round > MAX_ROUNDS {
            let _ = tx.send(ApiEvent::Note(format!("tool loop exceeded {MAX_ROUNDS} rounds, stopping")));
            msgs.push(Message::new(Role::Assistant, "stopped: tool loop limit reached"));
            let _ = tx.send(ApiEvent::Done {
                text: "stopped: tool loop limit reached".into(),
                messages: msgs,
            });
            return Ok(());
        }
        let reply = provider.chat(&req, &tx).await?;
        if reply.calls.is_empty() {
            msgs.push(Message::new(Role::Assistant, reply.text.clone()));
            let _ = tx.send(ApiEvent::Done {
                text: reply.text,
                messages: msgs,
            });
            return Ok(());
        }
        msgs.push(Message::new(Role::Assistant, reply.text.clone()).with_calls(reply.calls.clone()));
        req.messages = msgs.clone();
        for call in reply.calls {
            tx.send(ApiEvent::Tool {
                name: call.name.clone(),
                detail: tools::detail(&call.name, &call.args),
                diff: tools::preview(&call.name, &call.args),
            })
            .map_err(|_| anyhow!("closed"))?;
            if tools::needs_confirm(&call.name) && !allow_all.load(Ordering::Relaxed) {
                let (otx, orx) = oneshot::channel();
                tx.send(ApiEvent::Confirm {
                    name: call.name.clone(),
                    args: call.args.clone(),
                    rx: otx,
                })
                .map_err(|_| anyhow!("closed"))?;
                if !orx.await.unwrap_or(false) {
                    msgs.push(Message::tool(&call.id, "user denied this action"));
                    req.messages = msgs.clone();
                    continue;
                }
            }
            let out = match tools::execute(&call.name, &call.args, mcp.as_deref()).await {
                Ok(o) => o,
                Err(e) => format!("error: {e:#}"),
            };
            let summary: String = out
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(70)
                .collect();
            let _ = tx.send(ApiEvent::Note(summary));
            msgs.push(Message::tool(&call.id, out));
            req.messages = msgs.clone();
        }
    }
}
