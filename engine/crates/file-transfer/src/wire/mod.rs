//! The messages of file transfer.
//!
//! It covers the `PERMISSIONS` and `FILE_SHARE` requests and how their replies are read, the
//! phone's `/fileshare_android2cdeh_phase1` request and its answer, the `file_transfer.v1` control
//! messages on the channel, and the rules for received file names.

use prost::Message;

use phonelink_proto::file_transfer::v1::{
    FileShareContentMessage, FileShareFileStatus, FileShareMessage, FileTransferRequestMessage,
    FileTransferRequestType, FileTransferResponseMessage, NanoDelegateInitRequest, NanoDelegateInitResponse,
    NanoDelegateInitResponseStatus,
};
use phonelink_proto::permissions::v1::PermissionsResponseMessage;

/// The MSAEP tag of the phone's permission publication, which carries its context handle.
pub const PERMISSIONS_TAG: i16 = 3;

/// The PC's enrollment capability that makes the phone send files to it on this path.
pub const CAPABILITY: &str = "FILE_TRANSFER";

/// The path of the phone's receive request.
pub const RECEIVE_PATH: &str = "/fileshare_android2cdeh_phase1";

/// The `NamedStreams` scenario of a send (PC → phone).
pub const SEND_SCENARIO: &str = "fileShare_FDF73A34-0C30-4CC0-8165-87FC30DB1E35";
/// The `NamedStreams` scenario of a receive (phone → PC).
pub const RECEIVE_SCENARIO: &str = "fileShare_2B0F3A34-0C30-4CC0-8165-87FC30DB1E35";

/// The manifest metadata key of a pushed file's name.
pub const FILE_NAME_METADATA: &str = "fileShareDetailItemFileName";

/// `FileShareMessage.scenario` values.
pub mod scenario {
    pub const TO_PHONE: &str = "FileTransferToPhone";
    pub const TO_PHONE_FILE: &str = "FileTransferToPhoneFile";
    pub const TO_PHONE_COMPLETED: &str = "FileTransferToPhoneCompleted";
    pub const TO_PHONE_REJECTED: &str = "FileTransferToPhoneRejected";
    pub const TO_PC: &str = "FileTransferToPc";
    pub const TO_PC_CANCEL: &str = "FileTransferToPcCancel";
    pub const TO_PC_COMPLETED: &str = "FileTransferToPcCompleted";
}

/// The trace-state names (`ypScenarioId`, `ypTriggerId`) of each request.
pub mod trace_name {
    pub const REQUEST_PERMISSION: &str = "CDEH.FileShare.RequestPermission";
    pub const PROBE: (&str, &str) = ("CDEH.FileShare.SendDrmRequestForPermission", "ShareServiceStarted");
}

/// What made the PC send a permission request; the phone reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionTrigger {
    /// The silent check before a send.
    CheckFilePermission,
    /// The grant-permission notification's action.
    UserGrantFilesPermission,
    /// The request that makes the phone publish its context handle.
    GetContextHandle,
}

impl PermissionTrigger {
    pub fn wire(self) -> &'static str {
        match self {
            Self::CheckFilePermission => "CheckFilePermission",
            Self::UserGrantFilesPermission => "UserGrantFilesPermission",
            Self::GetContextHandle => "GetContextHandle",
        }
    }
}

/// `FileTransferRequestMessage { request_type: PERMISSION }`.
pub fn probe_request() -> Vec<u8> {
    FileTransferRequestMessage { request_type: FileTransferRequestType::Permission as i32 }.encode_to_vec()
}

/// The `FileTransferResponseMessage` status of a probe reply, as its enum name.
pub fn probe_status(payload: &[u8]) -> Result<String, prost::DecodeError> {
    let reply = FileTransferResponseMessage::decode(payload)?;
    Ok(match phonelink_proto::file_transfer::v1::FileTransferResponseStatus::try_from(reply.status) {
        Ok(s) => s.as_str_name().to_owned(),
        Err(_) => format!("unknown status {}", reply.status),
    })
}

/// The publication's `PermissionsResponseMessage` parses (its content is not used).
pub fn parse_publication(additional: &[u8]) -> Result<(), prost::DecodeError> {
    PermissionsResponseMessage::decode(additional).map(|_| ())
}

/// How the PC answers a receive request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveRequest {
    /// Answer with this `NanoDelegateInitResponse` and stop.
    Refuse(Vec<u8>),
    /// The request is well formed: its session id.
    Session(String),
}

fn init_response(session: &str, status: NanoDelegateInitResponseStatus, message: &str) -> Vec<u8> {
    NanoDelegateInitResponse {
        file_transfer_session_id: session.to_owned(),
        status: status as i32,
        message: message.to_owned(),
    }
    .encode_to_vec()
}

/// The `OK` answer for `session`.
pub fn init_ok(session: &str) -> Vec<u8> {
    init_response(session, NanoDelegateInitResponseStatus::Ok, "")
}

/// Checks a receive request's type, path and payload, in that order.
pub fn receive_request(is_get: bool, path: &str, payload: &[u8]) -> ReceiveRequest {
    use NanoDelegateInitResponseStatus as S;
    if !is_get {
        return ReceiveRequest::Refuse(init_response("", S::InvalidDeviceResourceRequestType, ""));
    }
    if path.is_empty() || !path.starts_with(RECEIVE_PATH) {
        return ReceiveRequest::Refuse(init_response("", S::InvalidDeviceResourceRequestPath, ""));
    }
    if payload.is_empty() {
        return ReceiveRequest::Refuse(init_response("", S::EmptyPayload, ""));
    }

    match NanoDelegateInitRequest::decode(payload) {
        Ok(r) => ReceiveRequest::Session(r.file_transfer_session_id),
        Err(e) => ReceiveRequest::Refuse(init_response("", S::UnrecognizedPayload, &e.to_string())),
    }
}

/// One file of a control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub file_id: String,
    pub file_name: String,
    pub file_size: i64,
    pub status: FileShareFileStatus,
}

impl FileEntry {
    fn to_wire(&self) -> FileShareContentMessage {
        FileShareContentMessage {
            file_id: self.file_id.clone(),
            file_name: self.file_name.clone(),
            file_size: self.file_size,
            file_hash: String::new(),
            file_status: self.status as i32,
        }
    }

    fn from_wire(c: &FileShareContentMessage) -> Self {
        Self {
            file_id: c.file_id.clone(),
            file_name: c.file_name.clone(),
            file_size: c.file_size,
            status: FileShareFileStatus::try_from(c.file_status).unwrap_or(FileShareFileStatus::Unspecified),
        }
    }
}

/// A control message as the feature reads and writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub transfer_id: String,
    pub scenario: String,
    pub sender_name: String,
    pub received_file_count: i32,
    pub content: Vec<FileEntry>,
    pub is_concurrent_mode: bool,
}

impl Control {
    pub fn encode(&self) -> Vec<u8> {
        FileShareMessage {
            transfer_id: self.transfer_id.clone(),
            scenario: self.scenario.clone(),
            sender_name: self.sender_name.clone(),
            received_file_count: self.received_file_count,
            content: self.content.iter().map(FileEntry::to_wire).collect(),
            pc_folder: String::new(),
            session_try_count: 1,
            is_concurrent_mode: self.is_concurrent_mode,
        }
        .encode_to_vec()
    }

    pub fn decode(body: &[u8]) -> Result<Self, prost::DecodeError> {
        let m = FileShareMessage::decode(body)?;
        Ok(Self {
            transfer_id: m.transfer_id,
            scenario: m.scenario,
            sender_name: m.sender_name,
            received_file_count: m.received_file_count,
            content: m.content.iter().map(FileEntry::from_wire).collect(),
            is_concurrent_mode: m.is_concurrent_mode,
        })
    }
}

/// Characters a received file name may not contain (besides U+0000–U+001F).
const INVALID_NAME_CHARS: &[char] = &['"', '<', '>', '|', ':', '*', '?', '\\', '/'];

/// Whether `name` is acceptable as the leaf name of a received file: not empty or white space,
/// one path component, none of the invalid characters, not ending in a space or `.`.
pub fn valid_file_name(name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    if name.chars().any(|c| INVALID_NAME_CHARS.contains(&c) || u32::from(c) < 0x20) {
        return false;
    }
    !(name.ends_with(' ') || name.ends_with('.'))
}

/// The `n`-th candidate for `name` (`0` = the name itself): `"<stem> (n)<ext>"`, where `<ext>` runs from
/// the last `.` to the end — also when that `.` is the first character — and is empty without a `.`.
pub fn candidate(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(at) => name.split_at(at),
        None => (name, ""),
    };
    format!("{stem} ({n}){ext}")
}

#[cfg(test)]
mod tests;
