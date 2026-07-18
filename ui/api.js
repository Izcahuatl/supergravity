// Thin wrappers over the Tauri bridge.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

export const api = {
  getInitialState: () => invoke("get_initial_state"),
  setUiState: (lastWorkspaceId, lastConversationId) =>
    invoke("set_ui_state", { lastWorkspaceId, lastConversationId }),
  listWorkspaces: () => invoke("list_workspaces"),
  addWorkspace: (name, path) => invoke("add_workspace", { name, path }),
  removeWorkspace: (id) => invoke("remove_workspace", { id }),
  listConversations: (workspaceId) => invoke("list_conversations", { workspaceId }),
  createConversation: (workspaceId, title, providerId, model) =>
    invoke("create_conversation", { workspaceId, title, providerId, model }),
  renameConversation: (id, title) => invoke("rename_conversation", { id, title }),
  deleteConversation: (id) => invoke("delete_conversation", { id }),
  getMessages: (conversationId) => invoke("get_messages", { conversationId }),
  sendMessage: (conversationId, text) => invoke("send_message", { conversationId, text }),
  cancelAgent: (conversationId) => invoke("cancel_agent", { conversationId }),
  resolveApproval: (conversationId, requestId, allow) =>
    invoke("resolve_approval", { conversationId, requestId, allow }),
  setApprovalMode: (conversationId, mode) => invoke("set_approval_mode", { conversationId, mode }),
  updateConversationModel: (conversationId, providerId, model) =>
    invoke("update_conversation_model", { conversationId, providerId, model }),
  listProviders: () => invoke("list_providers"),
  upsertProvider: (cfg) => invoke("upsert_provider", { cfg }),
  deleteProvider: (id) => invoke("delete_provider", { id }),
  setApiKey: (providerId, key) => invoke("set_api_key", { providerId, key }),
  deleteApiKey: (providerId) => invoke("delete_api_key", { providerId }),
  listLocalModels: (providerId) => invoke("list_local_models", { providerId }),
  onAgentEvent: (handler) => listen("agent-event", (e) => handler(e.payload)),
};
