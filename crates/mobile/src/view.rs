//! What the app publishes for drawing and what it accepts back: the whole surface Kotlin sees.
//!
//! Every text a person reads is decided on this side, so the screens hold no wording or rules of
//! their own beyond their labels. A view is published only when it changed.

/// A mail, by the list it lives in and its own id.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MailRef {
	pub list: String,
	pub element: String,
}

/// Where the person is. The last of the stack is on screen; below it is what Back returns to.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
	Starting,
	SignIn,
	Inbox,
	Conversation,
	Composer,
	Search,
	Settings,
	Rules,
	RuleEditor,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionKind {
	Connected,
	Working,
	Offline,
	Busy,
	SignedOut,
}

/// The header's second line: the unread count, or what Tern is doing or kept from doing.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct StatusView {
	pub kind: ConnectionKind,
	pub line: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SnackView {
	/// Changes with each message, so the same words shown twice count as two messages.
	pub id: u64,
	pub text: String,
	pub action: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ShellView {
	pub stack: Vec<Screen>,
	pub address: String,
	pub status: StatusView,
	pub snack: Option<SnackView>,
	pub pure_black: bool,
	pub match_wallpaper: bool,
	/// Bytes the folder menu offers to clean up, when it does.
	pub cleanup_offer: Option<String>,
	/// How often the system should wake the app to check for mail, in minutes; 0 for not at all.
	pub background_minutes: u32,
}

/// New mail for the system to say something about.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct MailNotice {
	/// Stays the same for a conversation, so news of it replaces what was shown before.
	pub id: i32,
	pub key: String,
	pub folder_id: String,
	pub who: String,
	pub subject: String,
	pub received_ms: i64,
}

/// What a check for mail in the background found.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BackgroundResult {
	pub notices: Vec<MailNotice>,
	/// What the group of them says, when there is more than one.
	pub summary: Option<String>,
	/// The check could not reach the server, so it is worth trying again before its next turn.
	pub retry: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SignInView {
	pub email: String,
	pub busy: bool,
	pub error: Option<String>,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderIcon {
	Inbox,
	Drafts,
	Sent,
	Archive,
	Spam,
	Trash,
	Folder,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct FolderView {
	pub id: String,
	pub name: String,
	pub icon: FolderIcon,
	pub unread: u32,
	pub selected: bool,
}

/// One conversation in a list.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RowView {
	pub key: String,
	pub folder_id: String,
	pub who: String,
	pub subject: String,
	pub date: String,
	pub unread: bool,
	pub pinned: bool,
	pub count: u32,
	pub has_attachments: bool,
	pub draft: bool,
	pub labels: Vec<String>,
	pub selected: bool,
	pub open: bool,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListState {
	Loading,
	Empty,
	Rows,
	Failed,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SelectionView {
	pub count: u32,
	/// Pin or unpin: unpin only when all of them are pinned.
	pub pin: bool,
	/// Mark as read or as unread: read when any of them is unread.
	pub mark_read: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ListView {
	pub folders: Vec<FolderView>,
	pub title: String,
	pub state: ListState,
	pub empty_title: String,
	pub empty_detail: Option<String>,
	pub rows: Vec<RowView>,
	/// There may be more to scroll to.
	pub more: bool,
	pub selection: Option<SelectionView>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SpanView {
	pub text: String,
	pub bold: bool,
	pub italic: bool,
	pub underline: bool,
	pub strike: bool,
	pub code: bool,
	pub link: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CellView {
	pub header: bool,
	pub spans: Vec<SpanView>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct TableRowView {
	pub cells: Vec<CellView>,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
	Text { spans: Vec<SpanView> },
	Heading { level: u8, spans: Vec<SpanView> },
	Pre { text: String },
	Rule,
	/// A picture the mail carries. No path means it is not here (yet).
	Image { path: Option<String>, alt: String },
	Table { rows: Vec<TableRowView> },
}

/// One drawn block of a mail. Quotes and lists are flattened into how deep each block sits.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BlockView {
	pub index: u32,
	pub kind: BlockKind,
	pub quote_depth: u32,
	pub list_depth: u32,
	pub marker: Option<String>,
	/// Picked with a long press: what a reply quotes.
	pub picked: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct AttachmentView {
	pub id: String,
	pub name: String,
	pub size: String,
	pub image: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CardView {
	pub id: MailRef,
	pub from: String,
	pub address: String,
	pub initials: String,
	pub recipients: Option<String>,
	pub date: String,
	pub snippet: Option<String>,
	pub expanded: bool,
	pub loading: bool,
	pub current: bool,
	pub draft: bool,
	pub unread: bool,
	pub blocks: Vec<BlockView>,
	pub attachments: Vec<AttachmentView>,
	/// Set when the mail leans on things Tern does not draw: where Tuta's web app shows it whole.
	pub web_url: Option<String>,
	pub notice: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ConversationView {
	pub key: String,
	pub subject: String,
	pub cards: Vec<CardView>,
	pub can_archive: bool,
	pub can_spam: bool,
	pub pinned: bool,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressField {
	To,
	Cc,
	Bcc,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerKind {
	Reply,
	ReplyAll,
	Forward,
}

/// The text of a composer as the person has it now.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ComposeText {
	pub to: String,
	pub cc: String,
	pub bcc: String,
	pub subject: String,
	pub body: String,
	pub from_index: u32,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SuggestionView {
	pub name: String,
	pub address: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ComposerAttachment {
	pub name: String,
	pub size: String,
	pub pending: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ComposerView {
	/// Changes when this side puts new text in the fields; the text fields take it then.
	pub revision: u64,
	pub title: String,
	pub text: ComposeText,
	pub addresses: Vec<String>,
	pub show_cc: bool,
	pub picked_who: Option<String>,
	pub picked_text: Option<String>,
	pub whole_label: Option<String>,
	pub whole_text: Option<String>,
	pub whole_included: bool,
	pub attachments: Vec<ComposerAttachment>,
	pub total: Option<String>,
	pub sending: bool,
	pub saving: bool,
	pub error: Option<String>,
	pub bad_to: bool,
	pub bad_cc: bool,
	pub bad_bcc: bool,
	pub suggest_field: Option<AddressField>,
	pub suggestions: Vec<SuggestionView>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SearchView {
	pub revision: u64,
	pub query: String,
	pub searching: bool,
	pub title: String,
	pub note: String,
	pub empty: bool,
	pub rows: Vec<RowView>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ChoiceView {
	pub value: String,
	pub label: String,
	pub selected: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SettingsView {
	pub address: String,
	pub keep_ready: Vec<ChoiceView>,
	pub keep_ready_line: String,
	pub forget_on_sign_out: bool,
	pub cleanup_limits: Vec<ChoiceView>,
	pub cleanup_limit_line: String,
	pub storage_line: String,
	pub semantic: bool,
	pub semantic_line: String,
	pub pure_black: bool,
	pub match_wallpaper: bool,
	pub rules_line: String,
	pub background: Vec<ChoiceView>,
	pub background_line: String,
	pub notifications: bool,
	pub notifications_line: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RuleRowView {
	pub index: u32,
	pub when: String,
	pub does: String,
	pub broken: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RulesView {
	pub rows: Vec<RuleRowView>,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinePart {
	From,
	To,
	Subject,
	Operator,
	Bracket,
	Value,
}

/// A stretch of a rule's line, by chars.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct LineSpan {
	pub start: u32,
	pub end: u32,
	pub part: LinePart,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulePiece {
	From,
	To,
	Subject,
	And,
	Or,
	Not,
	Brackets,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleDoes {
	Spam,
	Folder,
	Label,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTone {
	Neutral,
	Good,
	Bad,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct PreviewRow {
	pub who: String,
	pub subject: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RuleEditorView {
	pub revision: u64,
	pub cursor: u32,
	pub editing: bool,
	pub line: String,
	pub spans: Vec<LineSpan>,
	pub tone: PreviewTone,
	pub message: String,
	pub preview: Vec<PreviewRow>,
	pub more: Option<String>,
	pub does: RuleDoes,
	pub folders: Vec<String>,
	pub folder: String,
	pub labels: Vec<String>,
	pub label: String,
	pub can_save: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct LabelChoice {
	pub id: String,
	pub name: String,
	/// On every mail of the selection; `mixed` when only on some.
	pub on: bool,
	pub mixed: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SpamRow {
	pub id: MailRef,
	pub who: String,
	pub subject: String,
	pub date: String,
	pub picked: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CleanupChoice {
	pub part: CleanupPart,
	pub title: String,
	pub detail: String,
	pub size: String,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupPart {
	Older,
	Vectors,
	Model,
	All,
}

/// What opens over the screen.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SheetView {
	Labels { subject: String, choices: Vec<LabelChoice> },
	Spam { revision: u64, line: String, problem: Option<String>, rows: Vec<SpamRow>, count: String, keep_rule: bool, confirm: String, choices: Vec<ChoiceView> },
	Cleanup { total: String, choices: Vec<CleanupChoice> },
	Discard,
}

/// One part of what is on screen, changed.
#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum Update {
	Shell { view: ShellView },
	SignIn { view: SignInView },
	List { view: ListView },
	Conversation { view: Option<ConversationView> },
	Composer { view: Option<ComposerView> },
	Search { view: SearchView },
	Settings { view: SettingsView },
	Rules { view: RulesView },
	RuleEditor { view: Option<RuleEditorView> },
	Sheet { view: Option<SheetView> },
}

/// Something only the platform can do.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum Effect {
	OpenFile { path: String, mime: String },
	SaveFile { path: String, name: String, mime: String },
	OpenUrl { url: String },
	/// The app is on screen, so what the system says about new mail has been seen.
	ClearNotifications,
	Log { message: String },
}

/// What the person did.
#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum Action {
	SignIn { email: String, password: String },
	SignInAgain,
	SignOut,
	Back,
	Foreground,
	/// The app left the screen.
	Hidden,
	NetworkChanged { online: bool },
	SnackAction,
	SnackGone { id: u64 },

	OpenFolder { id: String },
	Refresh,
	LoadMore,
	OpenThread { key: String, folder_id: String },

	ToggleSelect { key: String },
	SelectAll,
	ClearSelection,
	SelectionPin,
	SelectionRead,
	SelectionTrash,
	SelectionSpam,
	SelectionLabels,

	ToggleCard { id: MailRef },
	TogglePick { id: MailRef, block: u32 },
	ConversationPin,
	ConversationUnread,
	ConversationArchive,
	ConversationTrash,
	ConversationSpam,
	ConversationLabels,
	OpenAttachment { mail: MailRef, attachment: String },
	SaveAttachment { mail: MailRef, attachment: String },
	OpenLink { url: String },

	Answer { mail: MailRef, kind: AnswerKind },
	EditDraft { mail: MailRef },
	ComposeNew,
	ComposeSend { text: ComposeText },
	ComposeClose { text: ComposeText },
	ComposeDiscard { text: ComposeText },
	ComposeDiscardConfirmed,
	ComposeAttach { name: String, mime: String, data: Vec<u8> },
	ComposeAttachFailed { name: String },
	ComposeRemoveAttachment { index: u32 },
	ComposeDropPicked,
	ComposeIncludeWhole { included: bool },
	ComposeShowCc,
	ComposeSuggest { field: AddressField, text: String },
	ComposeTakeSuggestion { text: ComposeText, field: AddressField, index: u32 },
	ComposeEdited,

	OpenSearch,
	SearchText { text: String },
	RuleInSearch,

	OpenSettings,
	SetKeepReady { value: String },
	SetForget { on: bool },
	SetCleanupLimit { value: String },
	SetSemantic { on: bool },
	SetPureBlack { on: bool },
	SetMatchWallpaper { on: bool },
	SetBackground { value: String },
	SetNotifications { on: bool },
	OpenCleanup,
	RunCleanup { older: bool, vectors: bool, model: bool, all: bool },

	OpenRules,
	OpenRule { index: Option<u32> },
	RuleLine { line: String },
	RulePut { piece: RulePiece, cursor: u32, selection_end: u32 },
	RuleSetDoes { does: RuleDoes },
	RuleSetFolder { name: String },
	RuleSetLabel { name: String },
	RuleSave,
	RuleDelete,

	ToggleLabel { id: String },
	CreateLabel { name: String },
	SpamLine { line: String },
	SpamUse { value: String },
	SpamToggle { id: MailRef },
	SpamSelectAll { on: bool },
	SpamKeepRule { on: bool },
	SpamConfirm,
	CloseSheet,
}
