use tauri::State;

use gipny_libcore::agent::BODY_OFF;
use gipny_libcore::{WireConsole, CONSOLE_COMMAND, CONSOLE_OFF};

use super::files::read_one_attachment;
use crate::core::AgentMaster;
use crate::{core_of, err, AppCtx};

#[tauri::command]
pub(crate) async fn get_agent_mode(ctx: State<'_, AppCtx>) -> Result<Option<AgentMaster>, String> {
    Ok(core_of(&ctx).await?.agent_master())
}

/// `Some(contact)` enables agent mode; `None` disables it and tells the master.
#[tauri::command]
pub(crate) async fn set_agent_mode(
    contact_id: Option<i64>,
    ctx: State<'_, AppCtx>,
) -> Result<(), String> {
    core_of(&ctx)
        .await?
        .set_agent_mode(contact_id)
        .await
        .map_err(err)
}

/// Master side: a line typed in the console, with any files to upload first.
#[tauri::command]
pub(crate) async fn send_console_command(
    contact_id: i64,
    body: String,
    paths: Vec<String>,
    ctx: State<'_, AppCtx>,
) -> Result<i64, String> {
    let core = core_of(&ctx).await?;
    let mut atts = Vec::with_capacity(paths.len());
    for p in &paths {
        atts.push(read_one_attachment(p, false)?);
    }
    core.send_console(contact_id, body, WireConsole::new(CONSOLE_COMMAND), atts)
        .await
        .map_err(err)
}

/// Master side: switch the agent's mode off remotely.
#[tauri::command]
pub(crate) async fn send_agent_off(contact_id: i64, ctx: State<'_, AppCtx>) -> Result<i64, String> {
    core_of(&ctx)
        .await?
        .send_console(
            contact_id,
            BODY_OFF.into(),
            WireConsole::new(CONSOLE_OFF),
            vec![],
        )
        .await
        .map_err(err)
}
