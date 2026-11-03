//! The Hands-Free Profile's AT dialogue as the HF sees it: the commands the Mac sends, and the lines
//! the phone's Audio Gateway sends back, split and parsed.
//!
//! Every parser takes one line without its `\r\n`. A line the parsers do not know is
//! [`Line::Other`]; nothing here fails on the phone's bytes.

/// The HF features the Mac announces in `AT+BRSF`.
///
/// Three-way calling (`0x02`), CLI presentation (`0x04`), enhanced call status (`0x20`) and enhanced
/// call control (`0x40`). Echo cancellation, voice recognition, remote volume and codec negotiation
/// are left out: each needs a voice link.
pub const HF_FEATURES: u32 = 0x66;

/// The AG feature bit for three-way calling.
pub const AG_THREE_WAY: u32 = 0x1;

/// The final result that completes a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Final {
    Ok,
    Error,
    /// `+CME ERROR: <n>`.
    CmeError(i32),
    NoCarrier,
    Busy,
    NoAnswer,
    Delayed,
}

impl Final {
    pub fn is_ok(&self) -> bool {
        matches!(self, Final::Ok)
    }
}

/// One `+CLCC` entry: a call the AG has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClccEntry {
    pub index: u32,
    /// `<dir>` 1: the call came in.
    pub incoming: bool,
    /// `<stat>`: 0 active, 1 held, 2 dialing, 3 alerting, 4 incoming, 5 waiting, 6 held by response and
    /// hold.
    pub status: u8,
    /// The number, when the AG gave one and it is not empty.
    pub number: Option<String>,
}

/// One line from the AG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Final(Final),
    /// `+BRSF: <AG features>`.
    Brsf(u32),
    /// `+CIND: ("<name>",(<range>)),…`: the indicator names in order.
    CindNames(Vec<String>),
    /// `+CIND: <v1>,<v2>,…`: the indicator values in order.
    CindValues(Vec<i32>),
    /// `+CHLD: (<modes>)`.
    Chld(Vec<String>),
    Clcc(ClccEntry),
    /// `+CIEV: <index>,<value>`; `index` is 1-based as the AG sends it.
    Ciev {
        index: usize,
        value: i32,
    },
    Ring,
    /// `+CLIP: "<number>",…`.
    Clip(Option<String>),
    /// `+CCWA: "<number>",…`.
    Ccwa(Option<String>),
    Other(String),
}

impl Line {
    /// The prefix that ties an information line to the command that asked for it.
    pub fn info_prefix(&self) -> Option<&'static str> {
        match self {
            Line::Brsf(_) => Some("+BRSF"),
            Line::CindNames(_) | Line::CindValues(_) => Some("+CIND"),
            Line::Chld(_) => Some("+CHLD"),
            Line::Clcc(_) => Some("+CLCC"),
            _ => None,
        }
    }
}

/// Parses one line.
pub fn parse(line: &str) -> Line {
    let line = line.trim();
    match line {
        "OK" => return Line::Final(Final::Ok),
        "ERROR" => return Line::Final(Final::Error),
        "NO CARRIER" => return Line::Final(Final::NoCarrier),
        "BUSY" => return Line::Final(Final::Busy),
        "NO ANSWER" => return Line::Final(Final::NoAnswer),
        "DELAYED" => return Line::Final(Final::Delayed),
        "RING" => return Line::Ring,
        _ => {}
    }

    let Some((name, rest)) = line.split_once(':') else { return Line::Other(line.to_string()) };
    let rest = rest.trim();
    let parsed = match name.trim() {
        "+CME ERROR" => rest.parse().ok().map(|n| Line::Final(Final::CmeError(n))),
        "+BRSF" => rest.parse().ok().map(Line::Brsf),
        "+CIND" if rest.starts_with('(') => Some(Line::CindNames(cind_names(rest))),
        "+CIND" => numbers(rest).map(Line::CindValues),
        "+CHLD" => Some(Line::Chld(chld_modes(rest))),
        "+CLCC" => clcc(rest).map(Line::Clcc),
        "+CIEV" => ciev(rest),
        "+CLIP" => Some(Line::Clip(first_number(rest))),
        "+CCWA" => Some(Line::Ccwa(first_number(rest))),
        _ => None,
    };
    parsed.unwrap_or_else(|| Line::Other(line.to_string()))
}

/// The fields of a result, split at the commas outside quotes, each trimmed.
fn fields(rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                field.push(c);
            }
            ',' if !quoted => out.push(std::mem::take(&mut field).trim().to_string()),
            _ => field.push(c),
        }
    }
    out.push(field.trim().to_string());
    out
}

fn unquote(field: &str) -> &str {
    field.trim().trim_matches('"')
}

fn numbers(rest: &str) -> Option<Vec<i32>> {
    fields(rest).iter().map(|f| f.parse().ok()).collect()
}

/// The quoted names of a `+CIND` range list, in order.
fn cind_names(rest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut quoted: Option<String> = None;
    for c in rest.chars() {
        match (c, quoted.as_mut()) {
            ('"', None) => quoted = Some(String::new()),
            ('"', Some(_)) => names.extend(quoted.take()),
            (_, Some(name)) => name.push(c),
            _ => {}
        }
    }
    names
}

/// The modes of `+CHLD: (0,1,1x,2,2x,3,4)`.
fn chld_modes(rest: &str) -> Vec<String> {
    rest.trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect()
}

fn clcc(rest: &str) -> Option<ClccEntry> {
    let f = fields(rest);
    if f.len() < 5 {
        return None;
    }

    let number = f.get(5).map(|n| unquote(n).to_string()).filter(|n| !n.is_empty());
    Some(ClccEntry {
        index: f[0].parse().ok()?,
        incoming: f[1].parse::<u8>().ok()? == 1,
        status: f[2].parse().ok()?,
        number,
    })
}

fn ciev(rest: &str) -> Option<Line> {
    let f = fields(rest);
    match f.as_slice() {
        [index, value] => Some(Line::Ciev { index: index.parse().ok()?, value: value.parse().ok()? }),
        _ => None,
    }
}

fn first_number(rest: &str) -> Option<String> {
    fields(rest).first().map(|n| unquote(n).to_string()).filter(|n| !n.is_empty())
}

/// A command's name without its arguments, for the log: `ATD` for a dial, else the text before `=`
/// or `?` (`AT+CHLD`). A dialed number never reaches the log.
pub fn command_name(command: &str) -> &str {
    if command.starts_with("ATD") {
        return "ATD";
    }
    command.split(['=', '?']).next().unwrap_or(command)
}

/// A line's name without its values, for the log: the text before `:` (`+BSIR`), or `other`.
pub fn line_name(line: &str) -> &str {
    match line.split_once(':') {
        Some((name, _)) if name.starts_with('+') && name.len() <= 16 => name,
        _ => "other",
    }
}

/// Splits the bytes the AG sends into lines: at every `\r` or `\n`, empty lines dropped. Bytes that
/// are not UTF-8 are replaced.
#[derive(Debug, Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\r' || b == b'\n' {
                if !self.pending.is_empty() {
                    lines.push(String::from_utf8_lossy(&self.pending).into_owned());
                    self.pending.clear();
                }
            } else {
                self.pending.push(b);
            }
        }
        lines
    }
}

/// The commands the Mac sends, each without its terminating `\r`.
pub mod command {
    use super::HF_FEATURES;

    pub fn brsf() -> String {
        format!("AT+BRSF={HF_FEATURES}")
    }
    pub const CIND_TEST: &str = "AT+CIND=?";
    pub const CIND_READ: &str = "AT+CIND?";
    pub const CMER: &str = "AT+CMER=3,0,0,1";
    pub const CHLD_TEST: &str = "AT+CHLD=?";
    pub const CLIP: &str = "AT+CLIP=1";
    pub const CCWA: &str = "AT+CCWA=1";
    pub const CLCC: &str = "AT+CLCC";
    pub const ANSWER: &str = "ATA";
    pub const HANG_UP: &str = "AT+CHUP";
    /// Releases the held calls, or rejects a waiting call (User Determined User Busy).
    pub const RELEASE_HELD_OR_REJECT_WAITING: &str = "AT+CHLD=0";
    /// Holds the active calls and accepts the held or waiting one.
    pub const HOLD_AND_ACCEPT_OTHER: &str = "AT+CHLD=2";

    /// `ATD<number>;`, `number` normalized ([`super::super::normalize_dialable`]).
    pub fn dial(number: &str) -> String {
        format!("ATD{number};")
    }
}

#[cfg(test)]
mod tests;
