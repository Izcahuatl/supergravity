use crate::core::error::Result;
use crate::core::types::{ApprovalMode, Message, ProviderConfig, ProviderKind, Role};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub provider_id: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A message with its row id and timestamp, for the bridge/UI layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: i64,
    pub role: Role,
    pub parts: Vec<crate::core::types::ContentPart>,
    pub created_at: i64,
}

/// SQLite-backed persistence. All methods are synchronous — async callers
/// (the Tauri bridge) MUST invoke them via `tokio::task::spawn_blocking`
/// or Tauri's sync-command mechanism, never directly on a runtime worker.
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn mode_str(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Manual => "manual",
        ApprovalMode::Auto => "auto",
    }
}

fn str_mode(s: &str) -> ApprovalMode {
    match s {
        "auto" => ApprovalMode::Auto,
        _ => ApprovalMode::Manual,
    }
}

fn kind_str(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAi => "open_ai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Gemini => "gemini",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OpenAiCompatible => "open_ai_compatible",
    }
}

fn str_kind(s: &str) -> ProviderKind {
    match s {
        "anthropic" => ProviderKind::Anthropic,
        "gemini" => ProviderKind::Gemini,
        "ollama" => ProviderKind::Ollama,
        "open_ai_compatible" => ProviderKind::OpenAiCompatible,
        _ => ProviderKind::OpenAi,
    }
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS workspaces(
                   id TEXT PRIMARY KEY,
                   name TEXT NOT NULL,
                   path TEXT NOT NULL UNIQUE,
                   created_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS conversations(
                   id TEXT PRIMARY KEY,
                   workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
                   title TEXT NOT NULL,
                   provider_id TEXT NOT NULL,
                   model TEXT NOT NULL,
                   approval_mode TEXT NOT NULL CHECK(approval_mode IN ('manual','auto')),
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS messages(
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                   role TEXT NOT NULL,
                   parts_json TEXT NOT NULL,
                   created_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS providers(
                   id TEXT PRIMARY KEY,
                   label TEXT NOT NULL,
                   kind TEXT NOT NULL,
                   base_url TEXT,
                   has_key INTEGER NOT NULL DEFAULT 0,
                   models_json TEXT NOT NULL,
                   extra_headers_json TEXT NOT NULL DEFAULT '[]'
                 );
                 PRAGMA user_version = 1;",
            )?;
        }
        if version < 2 {
            // Model enable/disable: everything starts OFF; users enable what they use.
            if table_exists(&conn, "providers") {
                conn.execute_batch("ALTER TABLE providers ADD COLUMN disabled_models_json TEXT NOT NULL DEFAULT '[]';")?;
            } else {
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS providers(
                       id TEXT PRIMARY KEY,
                       label TEXT NOT NULL,
                       kind TEXT NOT NULL,
                       base_url TEXT,
                       has_key INTEGER NOT NULL DEFAULT 0,
                       models_json TEXT NOT NULL,
                       extra_headers_json TEXT NOT NULL DEFAULT '[]',
                       disabled_models_json TEXT NOT NULL DEFAULT '[]'
                     );",
                )?;
            }
            conn.execute("UPDATE providers SET disabled_models_json = models_json", [])?;
            conn.execute_batch("PRAGMA user_version = 2;")?;
        }
        if version < 3 {
            // File checkpoints: pre-change snapshots so Rewind can restore the
            // workspace. content NULL = the file did not exist before the change.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS file_backups(
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                   after_message_id INTEGER NOT NULL,
                   path TEXT NOT NULL,
                   content BLOB,
                   created_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 3;",
            )?;
        }
        Ok(())
    }

    pub fn add_workspace(&self, name: &str, path: &str) -> Result<String> {
        let id = new_id();
        self.conn.lock().unwrap().execute(
            "INSERT INTO workspaces(id, name, path, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![id, name, path, now_ts()],
        )?;
        Ok(id)
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, name, path, created_at FROM workspaces ORDER BY created_at")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WorkspaceRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn remove_workspace(&self, id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_workspace(&self, id: &str) -> Result<WorkspaceRow> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT id, name, path, created_at FROM workspaces WHERE id = ?1",
            params![id],
            |r| {
                Ok(WorkspaceRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    created_at: r.get(3)?,
                })
            },
        )?)
    }

    pub fn get_provider(&self, id: &str) -> Result<ProviderConfig> {
        self.list_providers()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| crate::core::error::Error::Config(format!("unknown provider: {id}")))
    }

    pub fn update_conversation_model(
        &self,
        id: &str,
        provider_id: &str,
        model: &str,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE conversations SET provider_id = ?1, model = ?2, updated_at = ?3 WHERE id = ?4",
            params![provider_id, model, now_ts(), id],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn create_conversation(
        &self,
        workspace_id: &str,
        title: &str,
        provider_id: &str,
        model: &str,
        mode: ApprovalMode,
    ) -> Result<String> {
        let id = new_id();
        let now = now_ts();
        self.conn.lock().unwrap().execute(
            "INSERT INTO conversations(id, workspace_id, title, provider_id, model, approval_mode, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, workspace_id, title, provider_id, model, mode_str(mode), now, now],
        )?;
        Ok(id)
    }

    pub fn get_conversation(&self, id: &str) -> Result<ConversationRow> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT id, workspace_id, title, provider_id, model, approval_mode, created_at, updated_at
             FROM conversations WHERE id = ?1",
            params![id],
            |r| {
                Ok(ConversationRow {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    title: r.get(2)?,
                    provider_id: r.get(3)?,
                    model: r.get(4)?,
                    approval_mode: str_mode(&r.get::<_, String>(5)?),
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            },
        )?;
        Ok(row)
    }

    pub fn list_conversations(&self, workspace_id: &str) -> Result<Vec<ConversationRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, title, provider_id, model, approval_mode, created_at, updated_at
             FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at DESC, rowid DESC",
        )?;
        let rows = stmt
            .query_map(params![workspace_id], |r| {
                Ok(ConversationRow {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    title: r.get(2)?,
                    provider_id: r.get(3)?,
                    model: r.get(4)?,
                    approval_mode: str_mode(&r.get::<_, String>(5)?),
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now_ts(), id],
        )?;
        Ok(())
    }

    pub fn set_approval_mode(&self, id: &str, mode: ApprovalMode) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE conversations SET approval_mode = ?1, updated_at = ?2 WHERE id = ?3",
            params![mode_str(mode), now_ts(), id],
        )?;
        Ok(())
    }

    pub fn append_message(&self, conversation_id: &str, msg: &Message) -> Result<()> {
        let role = match msg.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        let parts_json = serde_json::to_string(&msg.parts)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages(conversation_id, role, parts_json, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![conversation_id, role, parts_json, now_ts()],
        )?;
        conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now_ts(), conversation_id],
        )?;
        Ok(())
    }

    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        Ok(self.get_message_rows(conversation_id)?.into_iter().map(|r| Message { role: r.role, parts: r.parts }).collect())
    }

    /// Timestamped messages for the bridge/UI layer.
    pub fn get_message_rows(&self, conversation_id: &str) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, role, parts_json, created_at FROM messages WHERE conversation_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![conversation_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for (id, role, parts_json, created_at) in rows {
            let role = match role.as_str() {
                "system" => Role::System,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let parts = serde_json::from_str(&parts_json)?;
            out.push(MessageRow {
                id,
                role,
                parts,
                created_at,
            });
        }
        Ok(out)
    }

    /// Rewind: drop the message with `message_id` and everything after it.
    pub fn rewind_messages(&self, conversation_id: &str, message_id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND id >= ?2",
            params![conversation_id, message_id],
        )?;
        Ok(())
    }

    /// Checkpoint: snapshot a file's current bytes before the agent mutates it
    /// (`None` = file did not exist). `after_message_id` ties the backup to the
    /// user turn whose run made the change.
    pub fn add_file_backup(
        &self,
        conversation_id: &str,
        after_message_id: i64,
        path: &str,
        content: Option<&[u8]>,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO file_backups(conversation_id, after_message_id, path, content, created_at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![conversation_id, after_message_id, path, content, now_ts()],
        )?;
        Ok(())
    }

    /// Id of the most recent message (0 when the conversation is empty).
    pub fn last_message_id(&self, conversation_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |r| r.get(0),
        )?)
    }

    /// Backups made at or after `message_id`, newest first (restore order).
    pub fn file_backups_from(
        &self,
        conversation_id: &str,
        message_id: i64,
    ) -> Result<Vec<(String, Option<Vec<u8>>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT path, content FROM file_backups WHERE conversation_id = ?1 AND after_message_id >= ?2 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![conversation_id, message_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Consume backups once a rewind has restored them.
    pub fn delete_file_backups_from(&self, conversation_id: &str, message_id: i64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM file_backups WHERE conversation_id = ?1 AND after_message_id >= ?2",
            params![conversation_id, message_id],
        )?;
        Ok(())
    }

    /// The oldest checkpoint for one file at one turn (its pre-turn state).
    pub fn file_backup_for(
        &self,
        conversation_id: &str,
        path: &str,
        after_message_id: i64,
    ) -> Result<Option<Option<Vec<u8>>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content FROM file_backups WHERE conversation_id = ?1 AND path = ?2 AND after_message_id = ?3 ORDER BY id ASC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![conversation_id, path, after_message_id])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get::<_, Option<Vec<u8>>>(0)?),
            None => None,
        })
    }

    /// Consume one file's checkpoints at one turn (after a per-file revert).
    pub fn delete_file_backups_for(
        &self,
        conversation_id: &str,
        path: &str,
        after_message_id: i64,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM file_backups WHERE conversation_id = ?1 AND path = ?2 AND after_message_id = ?3",
            params![conversation_id, path, after_message_id],
        )?;
        Ok(())
    }

    pub fn upsert_provider(&self, cfg: &ProviderConfig) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO providers(id, label, kind, base_url, has_key, models_json, extra_headers_json, disabled_models_json)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET label=excluded.label, kind=excluded.kind,
               base_url=excluded.base_url, has_key=excluded.has_key,
               models_json=excluded.models_json, extra_headers_json=excluded.extra_headers_json,
               disabled_models_json=excluded.disabled_models_json",
            params![
                cfg.id,
                cfg.label,
                kind_str(cfg.kind),
                cfg.base_url,
                cfg.has_key as i64,
                serde_json::to_string(&cfg.models)?,
                serde_json::to_string(&cfg.extra_headers)?,
                serde_json::to_string(&cfg.disabled_models)?,
            ],
        )?;
        Ok(())
    }

    pub fn list_providers(&self) -> Result<Vec<ProviderConfig>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, label, kind, base_url, has_key, models_json, extra_headers_json, disabled_models_json FROM providers ORDER BY label")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for (id, label, kind, base_url, has_key, models_json, extra_headers_json, disabled_models_json) in rows {
            out.push(ProviderConfig {
                id,
                label,
                kind: str_kind(&kind),
                base_url,
                has_key: has_key != 0,
                models: serde_json::from_str(&models_json)?,
                disabled_models: serde_json::from_str(&disabled_models_json)?,
                extra_headers: serde_json::from_str(&extra_headers_json)?,
            });
        }
        Ok(out)
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{
        ApprovalMode, ContentPart, Message, ProviderConfig, ProviderKind, Role,
    };

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn migrations_set_user_version() {
        let s = store();
        let v: i64 = s
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
    }

    #[test]
    fn workspace_crud() {
        let s = store();
        let id = s.add_workspace("proj", "/tmp/proj").unwrap();
        let list = s.list_workspaces().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "proj");
        assert_eq!(list[0].path, "/tmp/proj");
        s.remove_workspace(&id).unwrap();
        assert!(s.list_workspaces().unwrap().is_empty());
    }

    #[test]
    fn workspace_path_unique() {
        let s = store();
        s.add_workspace("a", "/tmp/x").unwrap();
        assert!(s.add_workspace("b", "/tmp/x").is_err());
    }

    #[test]
    fn conversation_lifecycle() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "Fix bug", "openai", "gpt-5", ApprovalMode::Manual)
            .unwrap();
        let conv = s.get_conversation(&cid).unwrap();
        assert_eq!(conv.title, "Fix bug");
        assert_eq!(conv.approval_mode, ApprovalMode::Manual);
        s.rename_conversation(&cid, "Fix login bug").unwrap();
        s.set_approval_mode(&cid, ApprovalMode::Auto).unwrap();
        let conv = s.get_conversation(&cid).unwrap();
        assert_eq!(conv.title, "Fix login bug");
        assert_eq!(conv.approval_mode, ApprovalMode::Auto);
        let list = s.list_conversations(&ws).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn messages_roundtrip_with_tool_parts() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        let msgs = vec![
            Message::text(Role::User, "read x"),
            Message {
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Text {
                        text: "reading".into(),
                    },
                    ContentPart::ToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        args_json: "{}".into(),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult {
                    tool_call_id: "c1".into(),
                    content: "data".into(),
                    is_error: false,
                }],
            },
        ];
        for m in &msgs {
            s.append_message(&cid, m).unwrap();
        }
        let back = s.get_messages(&cid).unwrap();
        assert_eq!(back, msgs);
    }

    #[test]
    fn file_backups_roundtrip_and_cascade() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        s.add_file_backup(&cid, 5, "a.txt", Some(b"v1")).unwrap();
        s.add_file_backup(&cid, 5, "a.txt", Some(b"v2")).unwrap();
        s.add_file_backup(&cid, 6, "new.txt", None).unwrap();
        // Newest first (restore order).
        let all = s.file_backups_from(&cid, 5).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0, "new.txt");
        assert_eq!(all[0].1, None);
        assert_eq!(all[2].1.as_deref(), Some(b"v1".as_slice()));
        // Boundary respected.
        assert_eq!(s.file_backups_from(&cid, 6).unwrap().len(), 1);
        s.delete_file_backups_from(&cid, 6).unwrap();
        assert_eq!(s.file_backups_from(&cid, 5).unwrap().len(), 2);
        // Cascades away with the conversation.
        s.delete_conversation(&cid).unwrap();
        assert_eq!(s.file_backups_from(&cid, 0).unwrap().len(), 0);
    }

    #[test]
    fn per_file_backup_lookup_and_consume() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        s.add_file_backup(&cid, 3, "a.txt", Some(b"orig")).unwrap();
        s.add_file_backup(&cid, 3, "a.txt", Some(b"mid")).unwrap();
        s.add_file_backup(&cid, 3, "b.txt", None).unwrap();
        // The lookup returns the OLDEST (pre-turn) content.
        let b = s.file_backup_for(&cid, "a.txt", 3).unwrap().unwrap();
        assert_eq!(b.as_deref(), Some(b"orig".as_slice()));
        assert!(s.file_backup_for(&cid, "c.txt", 3).unwrap().is_none());
        // Consuming affects only that file.
        s.delete_file_backups_for(&cid, "a.txt", 3).unwrap();
        assert!(s.file_backup_for(&cid, "a.txt", 3).unwrap().is_none());
        assert!(s.file_backup_for(&cid, "b.txt", 3).unwrap().is_some());
    }

    #[test]
    fn rewind_deletes_from_message_onward() {        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        s.append_message(&cid, &Message::text(Role::User, "first")).unwrap();
        s.append_message(&cid, &Message::text(Role::Assistant, "one")).unwrap();
        s.append_message(&cid, &Message::text(Role::User, "second")).unwrap();
        s.append_message(&cid, &Message::text(Role::Assistant, "two")).unwrap();
        let rows = s.get_message_rows(&cid).unwrap();
        assert_eq!(rows.len(), 4);
        // Row ids are stable and increasing — rewind at the second user turn.
        assert!(rows[0].id < rows[2].id);
        s.rewind_messages(&cid, rows[2].id).unwrap();
        let rows = s.get_message_rows(&cid).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].parts, Message::text(Role::Assistant, "one").parts);
        // Other conversations are untouched.
        let cid2 = s
            .create_conversation(&ws, "d", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        s.append_message(&cid2, &Message::text(Role::User, "keep")).unwrap();
        s.rewind_messages(&cid, rows[0].id).unwrap();
        assert_eq!(s.get_message_rows(&cid).unwrap().len(), 0);
        assert_eq!(s.get_message_rows(&cid2).unwrap().len(), 1);
    }

    #[test]
    fn workspace_delete_cascades() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        s.append_message(&cid, &Message::text(Role::User, "hi"))
            .unwrap();
        s.remove_workspace(&ws).unwrap();
        assert!(s.list_conversations(&ws).unwrap().is_empty());
        assert!(s.get_messages(&cid).unwrap().is_empty());
    }

    #[test]
    fn provider_upsert_list_delete() {
        let s = store();
        let cfg = ProviderConfig {
            id: "openai".into(),
            label: "OpenAI".into(),
            kind: ProviderKind::OpenAi,
            base_url: None,
            has_key: true,
            models: vec!["gpt-5".into()],
            disabled_models: vec![],
            extra_headers: vec![],
        };
        s.upsert_provider(&cfg).unwrap();
        let mut updated = cfg.clone();
        updated.has_key = false;
        updated.models = vec!["gpt-5".into(), "gpt-5-mini".into()];
        s.upsert_provider(&updated).unwrap();
        let list = s.list_providers().unwrap();
        assert_eq!(list.len(), 1, "upsert must not duplicate");
        assert_eq!(list[0], updated);
        s.delete_provider("openai").unwrap();
        assert!(s.list_providers().unwrap().is_empty());
    }

    #[test]
    fn get_workspace_and_provider_by_id() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let row = s.get_workspace(&ws).unwrap();
        assert_eq!(row.name, "proj");
        assert_eq!(row.path, "/tmp/proj");
        assert!(s.get_workspace("nope").is_err());
        let cfg = ProviderConfig {
            id: "openai".into(),
            label: "OpenAI".into(),
            kind: ProviderKind::OpenAi,
            base_url: None,
            has_key: false,
            models: vec!["gpt-5".into()],
            disabled_models: vec![],
            extra_headers: vec![],
        };
        s.upsert_provider(&cfg).unwrap();
        assert_eq!(s.get_provider("openai").unwrap(), cfg);
        assert!(s.get_provider("nope").is_err());
    }

    #[test]
    fn update_conversation_model_roundtrip() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "gpt-5", ApprovalMode::Auto)
            .unwrap();
        s.update_conversation_model(&cid, "anthropic", "claude-sonnet-4-5")
            .unwrap();
        let conv = s.get_conversation(&cid).unwrap();
        assert_eq!(conv.provider_id, "anthropic");
        assert_eq!(conv.model, "claude-sonnet-4-5");
    }

    #[test]
    fn delete_conversation_removes_it() {
        let s = store();
        let ws = s.add_workspace("proj", "/tmp/proj").unwrap();
        let cid = s
            .create_conversation(&ws, "c", "openai", "m", ApprovalMode::Auto)
            .unwrap();
        s.append_message(&cid, &Message::text(Role::User, "hi"))
            .unwrap();
        s.delete_conversation(&cid).unwrap();
        assert!(s.list_conversations(&ws).unwrap().is_empty());
        assert!(s.get_conversation(&cid).is_err());
        assert!(s.get_messages(&cid).unwrap().is_empty());
    }

    #[test]
    fn open_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/sg.db");
        let s = Store::open(&path).unwrap();
        assert!(path.exists());
        // the store is fully usable at the nested path
        s.add_workspace("proj", "/tmp/proj").unwrap();
        assert_eq!(s.list_workspaces().unwrap().len(), 1);
    }
}
