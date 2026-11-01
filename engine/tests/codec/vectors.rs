//! Byte vectors of the protocol and cross-checks of the hand-written codecs against the prost types
//! generated from the recovered schemas.

use phonelink_engine::codec::{dcg_packet, envelope, hub, valueset};
use phonelink_engine::proto::{device_info, pbvalueset, permissions, platform_sdk, remote_control};
use phonelink_engine::types::{HeaderMap, MessageType};
use prost::Message;

fn hex(s: &str) -> Vec<u8> {
    hex::decode(s.split_whitespace().collect::<String>()).unwrap()
}

/// The derived `ms-dcg` envelope test vector.
const VECTOR_ENVELOPE: &str = "
    01
    27 00 00 00
    02 00 00 00
    06 00 00 00  5F 72 6F 75 74 65
    02 00 00 00  2F 74
    0A 00 00 00  5F 72 65 71 75 65 73 74 49 64
    01 00 00 00  37
    0C 00 00 00 00 00 00 00
    0A 0A
      0A 01 61
      12 05
        08 0C
        6A 01 62";

#[test]
fn spec_envelope_vector_encodes_byte_exact() {
    let headers = HeaderMap::from_pairs([("_route", "/t"), ("_requestId", "7")]).unwrap();
    let payload = valueset::encode(&valueset::ValueSet::new().with("a", "b"));
    let bytes = envelope::encode(&headers, &payload).unwrap();
    assert_eq!(bytes, hex(VECTOR_ENVELOPE));
    assert_eq!(bytes.len(), 64);

    // With the default fragment size this is one fragment: FragmentId 1, FragmentCount 1, MessageType 0.
    let parts = dcg_packet::split_into_fragments(&bytes, 536_870_912);
    assert_eq!(parts.len(), 1);
    let fragment = dcg_packet::Fragment {
        version: dcg_packet::VERSION,
        session_id: "00000000-0000-0000-0000-000000000001".into(),
        sequence_number: 1,
        message_id: 1,
        message_type_raw: Some(MessageType::App.wire_value() as i32),
        fragment_id: 1,
        fragment_count: parts.len() as i32,
        raw: parts[0].to_vec(),
    };

    let packet = fragment.to_packet();
    let parsed = dcg_packet::parse_packet(&hub::MultiplexPacket::from_value(&packet.to_value()).unwrap()).unwrap();
    assert_eq!(parsed, dcg_packet::ParsedPacket::Fragment(fragment));
}

#[test]
fn spec_envelope_vector_decodes() {
    let env = envelope::decode(&hex(VECTOR_ENVELOPE)).unwrap();
    assert_eq!(env.headers.get("_route"), Some("/t"));
    assert_eq!(env.headers.get("_requestId"), Some("7"));
    let vs = valueset::decode(&env.payload).unwrap();
    assert_eq!(vs.str("a"), Some("b"));
}

#[test]
fn hand_written_valueset_matches_prost_schema() {
    use valueset::Value;
    let nested = valueset::ValueSet::new().with("n", 5i32);
    let ours = valueset::ValueSet::new()
        .with("s", "text")
        .with("i", -3i32)
        .with("d", 3.99f64)
        .with("b", true)
        .with("o", nested)
        .with("arr", Value::Int32Array(vec![1, -2, 3]))
        .with("strs", Value::StringArray(vec!["x".into(), "".into()]))
        .with("e", Value::Empty);

    let bytes = valueset::encode(&ours);
    let theirs = pbvalueset::ValueSet::decode(bytes.as_slice()).unwrap();
    let get = |k: &str| theirs.entries.get(k).unwrap();
    assert_eq!(get("s").value_type, pbvalueset::DataValueType::String as i32);
    assert_eq!(get("s").string_value, "text");
    assert_eq!(get("i").int32_value, -3);
    assert_eq!(get("d").double_value, 3.99);
    assert!(get("b").bool_value);
    assert_eq!(get("o").object_value.as_ref().unwrap().entries.get("n").unwrap().int32_value, 5);
    assert_eq!(get("arr").int32_array, vec![1, -2, 3]);
    assert_eq!(get("strs").string_array, vec!["x".to_string(), String::new()]);
    assert_eq!(get("e").value_type, pbvalueset::DataValueType::Empty as i32);

    // And the other way round: prost's encoding (packed repeated, defaults omitted) parses.
    let reencoded = theirs.encode_to_vec();
    let back = valueset::decode(&reencoded).unwrap();
    assert_eq!(back.str("s"), Some("text"));
    assert_eq!(back.get("arr"), Some(&Value::Int32Array(vec![1, -2, 3])));
    assert_eq!(back.object("o").unwrap().i32("n"), Some(5));
}

/// The device-status example's derived DRM bodies.
#[test]
fn derived_drm_bodies_match_prost_encoding() {
    let get = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Unknown as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Get as i32,
        payload: Vec::new(),
        resource_path: "/devicestatus".into(),
    };
    assert_eq!(get.encode_to_vec(), hex("08 04 10 01 22 0D 2F 64 65 76 69 63 65 73 74 61 74 75 73"));

    let dnd = device_info::v1::DeviceStatusRequestMessage { request_type: Some(2) };
    let update = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Unknown as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Update as i32,
        payload: dnd.encode_to_vec(),
        resource_path: "/devicestatus".into(),
    };
    assert_eq!(update.encode_to_vec(), hex("08 04 10 02 1A 02 08 02 22 0D 2F 64 65 76 69 63 65 73 74 61 74 75 73"));

    let perms = permissions::v1::PermissionsRequestMessage { feature_permissions: vec![5] };
    let sync = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Permissions as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Sync as i32,
        payload: perms.encode_to_vec(),
        resource_path: "/permissions".into(),
    };
    assert_eq!(sync.encode_to_vec(), hex("08 03 10 04 1A 03 0A 01 05 22 0C 2F 70 65 72 6D 69 73 73 69 6F 6E 73"));
}

/// The ring-my-phone example's derived bodies: the type-addressed ON and OFF the app sends, the
/// path-addressed form it does not send, and an `OK` reply.
#[test]
fn ring_my_phone_bodies_match_prost_encoding() {
    use phonelink_engine::features::device_status::{RingRequest, ring};
    use phonelink_engine::proto::ring_my_phone;
    let message = |resource_type: platform_sdk::v1::ProtoDeviceResourceType, request: RingRequest| {
        platform_sdk::v1::DeviceResourceManagerMessage {
            resource_type: resource_type as i32,
            request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Update as i32,
            payload: ring::request_payload(request),
            resource_path: "/ringmyphone".into(),
        }
        .encode_to_vec()
    };

    let on = ring::drm_request(RingRequest::On, std::time::Duration::from_secs(120));
    assert_eq!(on.payload, hex("08 01"));
    assert_eq!(
        message(platform_sdk::v1::ProtoDeviceResourceType::RingMyPhone, RingRequest::On),
        hex("08 07 10 02 1A 02 08 01 22 0C 2F 72 69 6E 67 6D 79 70 68 6F 6E 65")
    );
    assert_eq!(
        message(platform_sdk::v1::ProtoDeviceResourceType::RingMyPhone, RingRequest::Off),
        hex("08 07 10 02 1A 02 08 02 22 0C 2F 72 69 6E 67 6D 79 70 68 6F 6E 65")
    );
    assert_eq!(
        message(platform_sdk::v1::ProtoDeviceResourceType::Unknown, RingRequest::On),
        hex("08 04 10 02 1A 02 08 01 22 0C 2F 72 69 6E 67 6D 79 70 68 6F 6E 65")
    );

    let reply = platform_sdk::v1::DeviceResourceManagerResponseMessage {
        payload: ring_my_phone::v1::RingMyPhoneResponseMessage { status: 1 }.encode_to_vec(),
        response_type: Some(1),
    };
    assert_eq!(reply.encode_to_vec(), hex("0A 02 08 01 10 01"));
}

/// The remote-control page's vectors: the request and answer payloads, and the derived example's DRM
/// request and response bodies.
#[test]
fn remote_control_vectors() {
    use remote_control::v1::{RemoteControlRequestMessage, RemoteControlResponseMessage};
    let request = |t: i32| RemoteControlRequestMessage { control_type: t }.encode_to_vec();
    let answer = |r: i32, u: Option<bool>| RemoteControlResponseMessage { control_result: r, pc_update_available: u };

    assert_eq!(request(1), hex("08 01"));
    assert_eq!(request(4), hex("08 04"));
    assert!(request(0).is_empty());

    assert_eq!(answer(1, None).encode_to_vec(), hex("08 01"));
    assert_eq!(answer(2, None).encode_to_vec(), hex("08 02"));
    assert_eq!(answer(3, None).encode_to_vec(), hex("08 03"));
    assert_eq!(answer(1, Some(false)).encode_to_vec(), hex("08 01 10 00"));

    let drm = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Unknown as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Update as i32,
        payload: request(1),
        resource_path: "/remotecontrol".into(),
    };
    assert_eq!(drm.encode_to_vec(), hex("08 04 10 02 1A 02 08 01 22 0E 2F 72 65 6D 6F 74 65 63 6F 6E 74 72 6F 6C"));

    let response = platform_sdk::v1::DeviceResourceManagerResponseMessage {
        payload: answer(1, None).encode_to_vec(),
        response_type: Some(1),
    };
    assert_eq!(response.encode_to_vec(), hex("0A 02 08 01 10 01"));
}

/// The new-photo notifications example's derived bodies, built by the feature's own encoders.
#[test]
fn derived_remote_snipping_bodies() {
    use phonelink_engine::features::remote_snipping::wire;
    assert_eq!(wire::capabilities_request(), hex("08 01"));
    assert_eq!(wire::fetch_request(wire::Fetch::Thumbnail, 42), hex("08 02 10 2A"));
    assert_eq!(wire::fetch_request(wire::Fetch::Photo, 42), hex("08 03 10 2A"));
    assert_eq!(wire::photos_permission_request(), hex("0A 01 03"));

    let capability = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Media as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Get as i32,
        payload: wire::capabilities_request(),
        resource_path: "/media".into(),
    };
    assert_eq!(capability.encode_to_vec(), hex("08 06 10 01 1A 02 08 01 22 06 2F 6D 65 64 69 61"));

    let permission = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Permissions as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Sync as i32,
        payload: wire::photos_permission_request(),
        resource_path: "/permissions".into(),
    };
    assert_eq!(permission.encode_to_vec(), hex("08 03 10 04 1A 03 0A 01 03 22 0C 2F 70 65 72 6D 69 73 73 69 6F 6E 73"));
}

/// The handshake the ASP.NET Core `SignalR` client library sends by default.
#[test]
fn handshake_vector() {
    assert_eq!(
        hub::HANDSHAKE_REQUEST,
        hex("7B 22 70 72 6F 74 6F 63 6F 6C 22 3A 22 6D 65 73 73 61 67 65 70 61 63 6B 22 2C 22 76 65 72 73 69 6F 6E 22 3A 32 7D 1E")
            .as_slice()
    );
}

/// `/SessionValidation`: the baseline request.
#[test]
fn session_validation_baseline_request() {
    let req = platform_sdk::v1::ProtoPlatformSessionValidationRequest {
        platform_capabilities: vec![platform_sdk::v1::ProtoPlatformCapabilities::SessionValidation as i32],
    };
    // Packed repeated enum: field 1, length 1, value 1.
    assert_eq!(req.encode_to_vec(), vec![0x0A, 0x01, 0x01]);
}

/// Every `DataValueType` through the hand-written codec and the prost types generated from the
/// recovered `valueset.proto`, in both directions, so a wrong field number or wire type for any
/// type fails (the App payload: `ValueSet` ⇄ `pbvalueset`).
#[test]
#[expect(clippy::too_many_lines, reason = "one table of every value type")]
fn every_value_type_matches_the_recovered_schema_both_ways() {
    use pbvalueset::{DataValueType as T, DataVariant, Point as PPoint, Rect as PRect, Size as PSize};
    use valueset::{Point, Rect, Size, Value};
    let guid = uuid::Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
    let g = guid.hyphenated().to_string();

    let nested_ours = valueset::ValueSet::new().with("n", 5i32);
    let mut nested_theirs = pbvalueset::ValueSet::default();
    nested_theirs
        .entries
        .insert("n".into(), DataVariant { value_type: T::Int32 as i32, int32_value: 5, ..Default::default() });

    let dv = |t: T| DataVariant { value_type: t as i32, ..Default::default() };
    let cases: Vec<(Value, DataVariant)> = vec![
        (Value::UInt8(200), DataVariant { uint8_value: 200, ..dv(T::UInt8) }),
        (Value::Int16(-2), DataVariant { int16_value: -2, ..dv(T::Int16) }),
        (Value::UInt16(65535), DataVariant { uint16_value: 65535, ..dv(T::UInt16) }),
        (Value::Int32(-7), DataVariant { int32_value: -7, ..dv(T::Int32) }),
        (Value::UInt32(u32::MAX), DataVariant { uint32_value: u32::MAX, ..dv(T::UInt32) }),
        (Value::Int64(-9), DataVariant { int64_value: -9, ..dv(T::Int64) }),
        (Value::UInt64(u64::MAX), DataVariant { uint64_value: u64::MAX, ..dv(T::UInt64) }),
        (Value::Single(1.5), DataVariant { single_value: 1.5, ..dv(T::Single) }),
        (Value::Double(3.99), DataVariant { double_value: 3.99, ..dv(T::Double) }),
        (Value::Char16(0x263A), DataVariant { char16_value: 0x263A, ..dv(T::Char16) }),
        (Value::Bool(true), DataVariant { bool_value: true, ..dv(T::Bool) }),
        (Value::String("héllo".into()), DataVariant { string_value: "héllo".into(), ..dv(T::String) }),
        (Value::Guid(guid), DataVariant { guid_value: g.clone(), ..dv(T::Guid) }),
        (
            Value::Point(Point { x: 1.0, y: -2.0 }),
            DataVariant { point_value: Some(PPoint { x: 1.0, y: -2.0 }), ..dv(T::Point) },
        ),
        (
            Value::Size(Size { width: 3.0, height: 4.0 }),
            DataVariant { size_value: Some(PSize { width: 3.0, height: 4.0 }), ..dv(T::Size) },
        ),
        (
            Value::Rect(Rect { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }),
            DataVariant { rect_value: Some(PRect { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }), ..dv(T::Rect) },
        ),
        (
            Value::Object(nested_ours.clone()),
            DataVariant { object_value: Some(nested_theirs.clone()), ..dv(T::Object) },
        ),
        (
            Value::DateTime(133_000_000_000_000_000),
            DataVariant { datetime_value: 133_000_000_000_000_000, ..dv(T::DateTime) },
        ),
        (Value::TimeSpan(10_000_000), DataVariant { timespan_value: 10_000_000, ..dv(T::TimeSpan) }),
        (Value::UInt8Array(vec![1, 2, 3]), DataVariant { uint8_array: vec![1, 2, 3], ..dv(T::UInt8Array) }),
        (Value::Int16Array(vec![-1, 2]), DataVariant { int16_array: vec![-1, 2], ..dv(T::Int16Array) }),
        (Value::UInt16Array(vec![1, 65535]), DataVariant { uint16_array: vec![1, 65535], ..dv(T::UInt16Array) }),
        (Value::Int32Array(vec![-1, 5]), DataVariant { int32_array: vec![-1, 5], ..dv(T::Int32Array) }),
        (Value::UInt32Array(vec![0, u32::MAX]), DataVariant { uint32_array: vec![0, u32::MAX], ..dv(T::UInt32Array) }),
        (Value::Int64Array(vec![-5, i64::MAX]), DataVariant { int64_array: vec![-5, i64::MAX], ..dv(T::Int64Array) }),
        (Value::UInt64Array(vec![1, u64::MAX]), DataVariant { uint64_array: vec![1, u64::MAX], ..dv(T::UInt64Array) }),
        (Value::SingleArray(vec![0.5, -1.0]), DataVariant { single_array: vec![0.5, -1.0], ..dv(T::SingleArray) }),
        (Value::DoubleArray(vec![0.25, 1e300]), DataVariant { double_array: vec![0.25, 1e300], ..dv(T::DoubleArray) }),
        (Value::Char16Array(vec![65, 66]), DataVariant { char16_array: vec![65, 66], ..dv(T::Char16Array) }),
        (
            Value::BoolArray(vec![true, false, true]),
            DataVariant { bool_array: vec![true, false, true], ..dv(T::BoolArray) },
        ),
        (
            Value::StringArray(vec!["a".into(), String::new()]),
            DataVariant { string_array: vec!["a".into(), String::new()], ..dv(T::StringArray) },
        ),
        (
            Value::GuidArray(vec![guid, uuid::Uuid::nil()]),
            DataVariant { guid_array: vec![g.clone(), uuid::Uuid::nil().hyphenated().to_string()], ..dv(T::GuidArray) },
        ),
        (
            Value::PointArray(vec![Point { x: 1.0, y: 1.0 }, Point::default()]),
            DataVariant { point_array: vec![PPoint { x: 1.0, y: 1.0 }, PPoint::default()], ..dv(T::PointArray) },
        ),
        (
            Value::SizeArray(vec![Size { width: 1.0, height: 2.0 }]),
            DataVariant { size_array: vec![PSize { width: 1.0, height: 2.0 }], ..dv(T::SizeArray) },
        ),
        (
            Value::RectArray(vec![Rect { x: 1.0, y: 0.0, width: 2.0, height: 3.0 }]),
            DataVariant { rect_array: vec![PRect { x: 1.0, y: 0.0, width: 2.0, height: 3.0 }], ..dv(T::RectArray) },
        ),
        (
            Value::ObjectArray(vec![nested_ours.clone(), valueset::ValueSet::new()]),
            DataVariant {
                object_array: vec![nested_theirs.clone(), pbvalueset::ValueSet::default()],
                ..dv(T::ObjectArray)
            },
        ),
        (Value::DateTimeArray(vec![1, 2]), DataVariant { datetime_array: vec![1, 2], ..dv(T::DateTimeArray) }),
        (Value::TimeSpanArray(vec![3]), DataVariant { timespan_array: vec![3], ..dv(T::TimeSpanArray) }),
        (Value::Empty, dv(T::Empty)),
    ];
    assert_eq!(cases.len(), 39, "every DataValueType but Invalid");

    let mut ours = valueset::ValueSet::new();
    let mut theirs = pbvalueset::ValueSet::default();
    for (i, (value, variant)) in cases.iter().enumerate() {
        ours.insert(format!("k{i}"), value.clone());
        theirs.entries.insert(format!("k{i}"), variant.clone());
    }

    // Ours → prost.
    let decoded = pbvalueset::ValueSet::decode(valueset::encode(&ours).as_slice()).unwrap();
    for (i, (value, variant)) in cases.iter().enumerate() {
        assert_eq!(decoded.entries.get(&format!("k{i}")), Some(variant), "{} written by the engine", value.type_name());
    }

    // Prost → ours.
    let back = valueset::decode(&theirs.encode_to_vec()).unwrap();
    for (i, (value, _)) in cases.iter().enumerate() {
        assert_eq!(back.get(&format!("k{i}")), Some(value), "{} read by the engine", value.type_name());
    }
}

/// The wallpaper pull with nothing stored, and the `/wallpaper` answers without a picture.
#[test]
fn wallpaper_vectors() {
    use device_info::v1::{
        WallpaperRequestMessage, WallpaperRequestMetadata, WallpaperResponseMessage, WallpaperResponseMetadata,
        WallpaperType,
    };
    let request = WallpaperRequestMessage {
        current_wallpapers: vec![
            WallpaperRequestMetadata { id: -1, wallpaper_type: WallpaperType::Homescreen as i32 },
            WallpaperRequestMetadata { id: -1, wallpaper_type: WallpaperType::Lockscreen as i32 },
        ],
    };

    let pull = platform_sdk::v1::DeviceResourceManagerMessage {
        resource_type: platform_sdk::v1::ProtoDeviceResourceType::Wallpaper as i32,
        request_type: platform_sdk::v1::ProtoDeviceResourceRequestType::Get as i32,
        payload: request.encode_to_vec(),
        resource_path: "/wallpaper".into(),
    }
    .encode_to_vec();
    assert_eq!(
        pull,
        hex("08 05  10 01
             1A 1E  0A 0D 08 FF FF FF FF FF FF FF FF FF 01 10 01
                    0A 0D 08 FF FF FF FF FF FF FF FF FF 01 10 02
             22 0A 2F 77 61 6C 6C 70 61 70 65 72")
    );
    assert_eq!(pull.len(), 48);

    let none = WallpaperResponseMessage { wallpapers: vec![WallpaperResponseMetadata::default()], status: 0 };
    assert_eq!(none.encode_to_vec(), hex("0A 00"));

    let unreadable = WallpaperResponseMessage {
        wallpapers: vec![WallpaperResponseMetadata {
            wallpaper_type: WallpaperType::Homescreen as i32,
            ..Default::default()
        }],
        status: 0,
    };
    assert_eq!(unreadable.encode_to_vec(), hex("0A 02 10 01"));
}
