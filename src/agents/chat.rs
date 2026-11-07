//! The chat model shared by every agent: what the feed shows, independent of
//! which integration produced it. Nothing here is synthesized: items come from
//! App Server notifications.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Command,
    Read,
    Search,
    Edit,
    Web,
    Task,
    Tool,
}

impl ActionKind {
    pub fn icon(self) -> &'static str {
        match self {
            ActionKind::Command => "terminal",
            ActionKind::Read => "file-text",
            ActionKind::Search => "search",
            ActionKind::Edit => "file-pen",
            ActionKind::Web => "globe",
            ActionKind::Task => "bot",
            ActionKind::Tool => "wrench",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionStatus {
    Running,
    Done,
    Failed(Option<i64>),
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    pub id: String,
    pub kind: ActionKind,
    pub title: String,
    /// The real command or input, shown when the row is expanded.
    pub detail: String,
    pub output: String,
    pub status: ActionStatus,
    pub paths: Vec<String>,
    pub exit_code: Option<i64>,
}

impl ActionItem {
    /// One line for the feed: "Running npm run dev", "Edited src/a.rs".
    pub fn label(&self) -> String {
        let running = self.status == ActionStatus::Running;
        let verb = match (self.kind, running) {
            (ActionKind::Command, true) => "Running",
            (ActionKind::Command, false) => "Ran",
            (ActionKind::Read, true) => "Reading",
            (ActionKind::Read, false) => "Read",
            (ActionKind::Search, true) => "Searching",
            (ActionKind::Search, false) => "Searched",
            (ActionKind::Edit, true) => "Editing",
            (ActionKind::Edit, false) => "Edited",
            (ActionKind::Web, true) => "Fetching",
            (ActionKind::Web, false) => "Fetched",
            (ActionKind::Task, true) => "Running",
            (ActionKind::Task, false) => "Ran",
            (ActionKind::Tool, true) => "Using",
            (ActionKind::Tool, false) => "Used",
        };
        format!("{verb} {}", self.title).trim_end().to_string()
    }
}

/// `zsh -lc "npm run dev"` reads as `npm run dev`.
pub fn display_command(command: &str) -> String {
    let parts = shlex::split(command).unwrap_or_default();
    if let [shell, flag, inner] = parts.as_slice()
        && matches!(flag.as_str(), "-c" | "-lc")
        && (shell.ends_with("sh"))
    {
        return inner.lines().next().unwrap_or("").to_string();
    }
    command.lines().next().unwrap_or("").to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatItem {
    User {
        text: String,
        attachments: Vec<String>,
        images: Vec<std::path::PathBuf>,
    },
    Agent {
        id: String,
        text: String,
    },
    Action(ActionItem),
    Note(String),
    Error(String),
}

/// What the header and the feed say about the chat right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatPhase {
    Setup,
    Connecting,
    Empty,
    Idle,
    Responding,
    AwaitingPermission,
    Stopping,
    Failed,
}

pub struct PhaseInput {
    pub ready: bool,
    pub connecting: bool,
    pub item_count: usize,
    pub running: bool,
    pub stopping: bool,
    pub pending: usize,
    pub last_is_error: bool,
}

pub fn phase(i: &PhaseInput) -> ChatPhase {
    if i.stopping {
        ChatPhase::Stopping
    } else if i.pending > 0 {
        ChatPhase::AwaitingPermission
    } else if i.running {
        ChatPhase::Responding
    } else if i.connecting {
        ChatPhase::Connecting
    } else if !i.ready {
        ChatPhase::Setup
    } else if i.last_is_error {
        ChatPhase::Failed
    } else if i.item_count == 0 {
        ChatPhase::Empty
    } else {
        ChatPhase::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_cover_every_state() {
        let base = || PhaseInput {
            ready: true,
            connecting: false,
            item_count: 1,
            running: false,
            stopping: false,
            pending: 0,
            last_is_error: false,
        };
        assert_eq!(phase(&base()), ChatPhase::Idle);
        assert_eq!(
            phase(&PhaseInput {
                item_count: 0,
                ..base()
            }),
            ChatPhase::Empty
        );
        assert_eq!(
            phase(&PhaseInput {
                ready: false,
                ..base()
            }),
            ChatPhase::Setup
        );
        assert_eq!(
            phase(&PhaseInput {
                ready: false,
                connecting: true,
                ..base()
            }),
            ChatPhase::Connecting
        );
        assert_eq!(
            phase(&PhaseInput {
                running: true,
                ..base()
            }),
            ChatPhase::Responding
        );
        assert_eq!(
            phase(&PhaseInput {
                running: true,
                pending: 1,
                ..base()
            }),
            ChatPhase::AwaitingPermission
        );
        assert_eq!(
            phase(&PhaseInput {
                running: true,
                stopping: true,
                ..base()
            }),
            ChatPhase::Stopping
        );
        assert_eq!(
            phase(&PhaseInput {
                last_is_error: true,
                ..base()
            }),
            ChatPhase::Failed
        );
    }

    #[test]
    fn shell_wrappers_are_not_shown() {
        assert_eq!(
            display_command("/bin/zsh -lc \"npm run dev\""),
            "npm run dev"
        );
        assert_eq!(display_command("bash -c 'ls'\n"), "ls");
        assert_eq!(display_command("npm run dev\nsecond"), "npm run dev");
    }
}
