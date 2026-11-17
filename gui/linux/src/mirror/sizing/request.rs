//! A window's `mirror.frame` query: the JSON it asks with, and the answer.

use serde_json::{Value, json};

use super::geometry::{Placement, Rect, Screen, Size};
use super::placement::{cascaded, restored};
use super::sizes::{default_size, height_changed, holds_navigation_bar, minimum_size, reshaped, same_aspect};

/// The aspect (width ÷ height) of a window whose video and phone resolution are not known.
pub(super) const DEFAULT_ASPECT: f64 = 0.5125;

/// Why a `mirror.frame` query was not answered: it lacks what its op needs.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum FrameRequestError {
    #[error("no op in {request}")]
    NoOp { request: Value },
    #[error("unknown op {op}")]
    UnknownOp { op: String },
    #[error("{key} is not a number in {value}")]
    NotANumber { key: &'static str, value: Value },
    #[error("{key} is not a list in {value}")]
    NotAList { key: &'static str, value: Value },
}

fn number(value: &Value, key: &'static str) -> Result<f64, FrameRequestError> {
    value[key].as_f64().ok_or_else(|| FrameRequestError::NotANumber { key, value: value.clone() })
}

fn size_of(value: &Value) -> Result<Size, FrameRequestError> {
    Ok(Size::new(number(value, "width")?, number(value, "height")?))
}

fn rect_of(value: &Value) -> Result<Rect, FrameRequestError> {
    Ok(Rect::new(number(value, "x")?, number(value, "y")?, number(value, "width")?, number(value, "height")?))
}

fn list_of<T>(
    value: &Value,
    key: &'static str,
    item: impl Fn(&Value) -> Result<T, FrameRequestError>,
) -> Result<Vec<T>, FrameRequestError> {
    value[key]
        .as_array()
        .ok_or_else(|| FrameRequestError::NotAList { key, value: value.clone() })?
        .iter()
        .map(item)
        .collect()
}

fn size_json(size: Size) -> Value {
    json!({ "width": size.width, "height": size.height })
}

fn rect_json(rect: Rect) -> Value {
    json!({ "x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height })
}

/// The answer to a window's `mirror.frame` query, whose `op` names what it asks for.
///
/// - `opening` (`aspect`, `visible`, `nonClient`): a new window's size, and the aspect it has (a
///   phone's usual one when `aspect` is null: the video's and the phone's sizes are not known).
/// - `minimum` (the same): the smallest window.
/// - `reshape` (`content`, `shapedFor`, `current`, `fallbackAspect`, `previous` — `{aspect, width,
///   height}` or null —, `visible`, `nonClient`): the size for a video of another aspect than the one
///   the window was shaped for, or null to keep the window.
/// - `holdsNavigationBar` (`current`, `nonClientWithoutBar`, `aspect`): whether the window holds it.
/// - `heightChanged` (`current`, `delta`, `aspect`, `visible`, `nonClient`): the size with the height
///   changed.
/// - `restore` (`saved`, `screens`): the kept frame, or null when it is on no screen.
/// - `cascade` (`frame`, `screens` — `{frame, visible}` each —, `peers`): the frame stepped away.
///
/// Sizes are `{width, height}`, rectangles `{x, y, width, height}`.
pub fn answer(request: &Value) -> Result<Value, FrameRequestError> {
    let op = request["op"].as_str().ok_or_else(|| FrameRequestError::NoOp { request: request.clone() })?;
    let visible = || size_of(&request["visible"]);
    let non_client = || size_of(&request["nonClient"]);
    let aspect = || match &request["aspect"] {
        Value::Null => Ok(DEFAULT_ASPECT),
        _ => number(request, "aspect"),
    };

    let value = match op {
        "opening" => {
            let aspect = aspect()?;
            let size = default_size(aspect, visible()?, non_client()?);
            json!({ "width": size.width, "height": size.height, "aspect": aspect })
        }
        "minimum" => size_json(minimum_size(aspect()?, visible()?, non_client()?)),
        "reshape" => {
            let previous = match &request["previous"] {
                Value::Null => None,
                previous => Some(Placement { aspect: number(previous, "aspect")?, size: size_of(previous)? }),
            };
            let content = size_of(&request["content"])?;
            // The user resizes the window freely, so only a video of another aspect than the one the
            // window was last shaped for reshapes it.
            let shaped_for = number(request, "shapedFor")?;
            if content.height > 0.0 && same_aspect(content.width / content.height, shaped_for) {
                return Ok(Value::Null);
            }
            let shaped = reshaped(
                content,
                size_of(&request["current"])?,
                number(request, "fallbackAspect")?,
                previous,
                visible()?,
                non_client()?,
            );
            shaped.map_or(Value::Null, size_json)
        }
        "holdsNavigationBar" => Value::Bool(holds_navigation_bar(
            size_of(&request["current"])?,
            size_of(&request["nonClientWithoutBar"])?,
            number(request, "aspect")?,
        )),
        "heightChanged" => size_json(height_changed(
            size_of(&request["current"])?,
            number(request, "delta")?,
            number(request, "aspect")?,
            visible()?,
            non_client()?,
        )),
        "restore" => {
            let screens = list_of(request, "screens", rect_of)?;
            restored(rect_of(&request["saved"])?, &screens).map_or(Value::Null, rect_json)
        }
        "cascade" => {
            let screens = list_of(request, "screens", |screen| {
                Ok(Screen { frame: rect_of(&screen["frame"])?, visible: rect_of(&screen["visible"])? })
            })?;
            let peers = list_of(request, "peers", rect_of)?;
            rect_json(cascaded(rect_of(&request["frame"])?, &screens, &peers))
        }
        other => return Err(FrameRequestError::UnknownOp { op: other.to_owned() }),
    };
    Ok(value)
}
