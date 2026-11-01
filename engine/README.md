# `engine/` — the protocol core

A Cargo workspace: the `phonelink-engine` package in this directory and the crates beneath it under
`crates/` (*Crate map*, below). `phonelink-engine` builds the library that `gui/` links through `uniffi`
as one static library, with every crate beneath it, and the headless `phonelink` CLI. It holds the
protocol layers and the features — notifications, media controls, clipboard, device status (with ring my
phone), remote control, messaging, contacts, photos, wallpaper, new-photo notifications (remote
snipping), instant hotspot, file transfer, phone files — and the `uniffi` surface the macOS app drives
them through (`api`). Toolchain: stable Rust; no `protoc`.

## Build and test

Every command is a recipe in [`justfile`](../justfile) (`just` in the repository root lists them): `just
engine-build`, `just engine-test`, `just engine-lint` (clippy with `-D warnings` over the tests and over the
app build, the layering check `scripts/layers.py`, `cargo fmt --check`), `just engine-fmt`,
`just engine-audit` (counts of the lints left off), `just engine-bindings`, which generates the Swift
bindings from the debug static library into `target/uniffi/`, and `just engine-bindings-diff`, which
compares them with a saved baseline (`just engine-bindings-baseline`).

`crates/proto/build.rs` compiles every schema under `crates/proto/proto/` with `protox` + `prost-build`
(no `protoc`). Each crate that exports `uniffi` items has its own Swift bindings (`<crate>.swift`,
`<crate>FFI.h`, `<crate>FFI.modulemap`); the generated Swift (`target/uniffi/`) is build output, and it
typechecks with the Command Line Tools' `swiftc -typecheck $(for m in *FFI.modulemap; do echo -Xcc
-fmodule-map-file=$m; done) -I . *.swift`. The macOS app's build generates its own copy
(`gui/scripts/build-engine.sh`, which joins the modulemaps into one and lists the crates the Xcode
project compiles), building the library in `target/app/`. The `phonelink-engine` package lists only
the `lib` crate type, so `cargo build` and `cargo test` link no
static library; the app's build and `just engine-bindings` ask for one with `cargo rustc -p
phonelink-engine --crate-type staticlib`. The bindgen binary builds in `target/bindgen/`: it builds the library again with the
`bindgen-cli` feature, and cargo names a static library without a hash, so a static library built with
other features in the same target directory would replace the one there. `.cargo/config.toml` sets
`MACOSX_DEPLOYMENT_TARGET` to the app's minimum (14.0), so the C dependencies (ring, SQLite) link into the
app without "built for newer macOS" warnings.

Every test is offline: servers are in-process fakes bound to `127.0.0.1`. The integration tests are one
test binary: `tests/main.rs` declares one folder per layer (`codec`, `auth`, `relay`, `dcg`, `platform`,
`nano`, `app`, `features/<feature>`, `engine`, `api`, `e2e`), each with a `mod.rs` listing its files, and
`tests/support/`, the worlds and fakes they share, once, so the crate and its dependencies link once rather
than once per file. They build the library with the `test-support` feature, which makes the inner layers
public to them at the paths `phonelink-engine` re-exports them by (`phonelink_engine::features::audio`
is the `phonelink-audio` crate). Run one file's tests with `cargo test --test engine <layer>::<file>::`
(for example `features::notifications::queue::`). Unit tests live in a `tests.rs` beside the module they
test, and each crate's are one test binary: `cargo test -p phonelink-clipboard` runs the clipboard's.

| Test files | Cover |
|---|---|
| `tests/codec/vectors.rs` | the protocol's byte vectors and the hand-written codecs against the generated prost types |
| `tests/auth/` (`support/auth_world/`: fake DCG, fake Microsoft account service and sign-in web view, wake hub) | sign-in, provisioning, tokens, trust, wake, the DCG REST resilience policy (`auth::rest_policy`; the other tests run the control plane with one attempt per call) |
| `tests/relay/`, `tests/dcg/` (`support/relay_world`, `support/fake_hub`) | the hub connection, presence, the fragment layer |
| `tests/platform/` (`support/platform_world`) | the router, the platform routes, the per-partner connection, the context handle's Nano exchange (initialize, connection information, the refused phone-initiated initialize, the drop) |
| `tests/nano/` (`support/nano_phone`: a scripted phone on 127.0.0.1 — an IpSrtp listener or a controlled ICE responder, the server halves of the MTU probe, keepalive and rate-control handshakes, the engine's own rate controller and channel types) | IpSrtp and IceSrtp connections and a message channel over them, the device connection's message channel, a platform channel's creation and refusal, its scoped messages and a blob stream, the close reasons (the phone's close, keepalive timeout, open timeout) |
| `tests/app/` (`support/app_world`) | App requests, the connect message, inbound dispatch, the phone record |
| `tests/e2e/stack.rs` (`support/scripted_phone`) | every layer wired by `EngineCore` against all the fakes: sign-in → enrollment → hub → wake → `/SessionValidation` → `/connect/v1` → phone traffic |
| `tests/features/shared.rs` (`support/feature_world`: a `FeatureEnv` over the App harness) | the shared feature pieces: permission flags with their feature defaults, the peer directory, the per-phone switch |
| `tests/features/notifications/` (`support/notifications_world`) | inbound batches, presentation, direct requests and full syncs, the action queue, per-app settings, the connect contribution |
| `tests/features/audio/` (`support/notifications_world` with audio installed) | media sessions from notification batches, the gates, the selection and the source-switch play, the `/command/audio` requests, the busy state |
| `tests/features/clipboard/` (`support/clipboard_world`: the platform world with a `FeatureEnv`, an in-memory pasteboard with a read hook, a vanishing device list, a fake codec) | announcements both ways, the content pull, the `/clipboard` receiver, the feature-state sync, the feature's stop |
| `tests/features/device_status/` (`support/device_status_world`) | the CDEH pull, push and `/remotedevicestatus`; the Phone Link contract; the DND and ringer controls; ring my phone (the capability, the control, the `RING_MY_PHONE` request and its outcomes, the countdown, the two-step stop); both contracts on one session |
| `tests/features/messaging/` (`support/messaging_world`) | the `messaging` exchanges and their validation, the connect contribution, `/chat/v1` requests and sending, new-message notifications |
| `tests/features/contacts/` (`support/contacts_world`) | `contactsV2` sync, the connect contribution, `/contacts/v2` and the contacts `/permission` |
| `tests/features/photos/` (`support/photos_world`) | the `mediaInfo` sync, the connect contribution, the fetch dispatchers, the delete and permission flows |
| `tests/features/remote_control/` (`support/remote_control_world`: the device-status session world, a recording power edge, a scripted device-list refresh) | the `/remotecontrol` receiver's actions, answers and gates, the refresh before a capability failure, the addresses that reach it; the `remoteControl` processor and its value rules; both through `EngineCore` with the cloud refresh. No test can lock, sleep, shut down or restart the machine: the engine has no implementation of the power edge of its own |
| `tests/features/wallpaper/` (`support/wallpaper_world`: the device-status session world with wallpaper installed and a desktop picture the test sets) | Phone Link's connect contribution and its gates, the `wallpaper` content; the Cross Device pull and its reply rules, `/wallpaper`, tag 5; the wallpaper shown |
| `tests/features/remote_snipping/` (`support/remote_snipping_world`: the platform world with an App layer, a scripted phone for `/DeviceResourceManager`, a recording notifier) | the capability request and its triggers, the tag-6 publication and the notification (rate limit, window, silent pop-up, expiry, activation), tag 3 and the permission request, which phones the feature covers; the photos notification skipped for a covered phone |
| `tests/features/file_transfer/` (`support/file_transfer_world`: the platform world with an App layer, a scripted phone for `/DeviceResourceManager`, a recording notifier, and channels opened as in-memory NamedStreams pairs whose phone end the test drives) | the `FILE_TRANSFER` capability, the `FILE_SHARE` probe, the settings; a send (the gate, the silent permission check and the grant-permission notification, the context handle from tag 3 or the `GetContextHandle` request and its 120 s wait, the channel's refusals, the file list, the phone's requests and the pushed bytes, completion, rejection, cancel, the share-menu notifications); a receive (the request's refusals and the busy and unknown-phone cases, the pull into the folder with name collisions and an empty file, the completion message, the received notification and its click, a bad file name, a cancel that deletes the partial file) |
| `tests/features/phone_files/` (reuses `support/file_transfer_world`, with its own in-memory files channels) | enablement from the phone's tag-4 configuration and the config sync; the full sync over `/DeviceResourceManager` filling the tree with synthetic folders; an incremental sync over the files-request channel and the change journal; the trace state of each `GET` over the relay; an expired sequence asked again at once; a download pulled into a temporary file with progress; an upload pushed with its manifest metadata and the phone's `create_file_response`; ignored names; thumbnails over the relay and their cache; create folder and rename; a folder delete as one batch ordered by leaf-name length and the recycle-bin notice; turning the feature off and on |
| `tests/features/phone_apps/` (`support/app_world`) | the `phoneApps` sync (metadata, phone-app items, recent items, completion), its malformed-content rejections, the connect contribution, the lists the UI shows, and the app names and icons other features read |
| `tests/features/bluetooth/` (`support/feature_world`) | the phone's Bluetooth device on its own: the names a paired device may carry, finding the paired phone, a pairing's early stops and its edge failures, the Bluetooth `/permission` request recording its decision |
| `tests/features/hotspot/` (`support/app_world`) | instant hotspot and the shared phone Bluetooth device: the connect's permission wait and pairing, the phone's Bluetooth name, one pairing at a time, the service-record refresh, a closed or silent side channel, the joined state a connect leaves and the disconnect |
| `tests/features/connected_camera/` (`support/camera_world`) | the sync and the context handle from tag 2, publishing the camera, opening a stream (permissions, the auto-prompt, the handle, the channel), the phone's control messages, frames, closing, unpublishing, the screen lock, tag 3, the network quality |
| `tests/features/screen_mirroring/` | screen mirroring end to end against an in-memory phone the test plays on every channel of an in-memory connection |
| `tests/cli/` | the `phonelink` binary run as a process, offline: `decode` printing a seam's JSON and failing on bad input, `status` and `store-keys` on an empty state directory |
| `tests/e2e/features.rs` | every layer and feature wired by `EngineCore`: the providers' connect keys and response, a notification batch presented, a CDEH device-status publication, a notification reply routed back to the phone |
| `tests/engine/capabilities.rs` | the `CLIPBOARD` capability through the engine: `EnrollDevice` follows the persisted clipboard switch, `SetDeviceInfo` follows its changes (also after a lagged event stream); the exact `metadata.capabilities` lists |
| `tests/engine/identities.rs` | the `PL` and `WEA` identities' own keys, capabilities and hub connections over one account; the `WEA` session stops on the Mac's lock and starts again (new session) on its unlock, not on a wake while locked |
| `tests/engine/session.rs` (`support/engine_world`: a started engine and a scripted phone that records what reaches it with its hub trace context) | every device-list refresh rewrites the phone record; the account gate while the MSA token cannot be refreshed; account removal stops the clipboard and the capability bridge and needs a rebuild, which the `uniffi` `Engine`'s next start performs; the foreground state and the activation and refresh connects on the wire; content types without a processor answered 2/1 with every feature installed; the account gate after the selected phone left the device list; a foreground report during the start; the per-process identifiers a rebuilt core keeps |
| `tests/engine/phone_selection.rs` (`support/engine_world`) | no phone is chosen automatically; the user's selection starts the phone session and the next start restores it; another phone can be selected while the engine runs; an id that names no linked phone is refused |
| `tests/engine/lifecycle.rs` | a start after `stop` reconnects; a failed feature install needs a rebuild; a notification response from a foreign thread runs in the engine's runtime |
| `tests/api/objects.rs` (`support/engine_world`) | the `uniffi` feature objects: `NotStarted` before a start and the same objects working after it; reading the selected phone; a pushed notification arriving as a typed event and listed, pinned and dismissed through the object; waking the Mac removing the phone with `SystemReset`, re-waking it and sending the unlock-or-wake connect |

## The CLI

```bash
cargo run --bin phonelink -- decode envelope <hex | @file>
cargo run --bin phonelink -- status
cargo run --bin phonelink -- session --tap <dir>
```

`decode` (layers `frames`, `hub`, `packet`, `envelope`, `valueset`) and `status` are offline. `sign-in`,
`enroll` and `session` contact Microsoft's production services and wake the phone. State lives in
`--state-dir` (`PHONELINK_STATE_DIR`), default `.local/port/` at the repository root (gitignored), one
`0600` file per key. The features' stores (notifications, messages, contacts, photos, wallpapers — phone content)
live in `--data-dir` (`PHONELINK_DATA_DIR`), default `.local/port-data/` (gitignored). The CLI's stand-ins for the
features' OS edges print system notifications to stderr and have no pasteboard, image codec, PC power or
network source, attachment imaging, desktop picture or Bluetooth, and refuse every remote-control action (lock, sleep, shut down,
restart). `session --tap <dir>` writes every decode-seam event as JSON lines,
unredacted except bearer tokens — give it a directory outside version control.

The macOS app writes the same tap when it is launched with `PHONELINK_TAP_DIR=<dir>` in its environment
(`tap-<unix seconds>.jsonl`, one file per engine core); unset, it writes none.

Started with `PHONELINK_NANO_PROBE` set, the app (or CLI) runs `nano::probe` once after the selected phone's
Cross Device session connects: CloudRelay, a candidate gathering with its servers, the device connection
and an InternalMessaging platform channel, each step logged with target `nano_probe`; it contacts
Microsoft's relay and wakes the phone. `PHONELINK_NANO_PROBE=relay` opens the device connection with
one attempt that offers and pairs only relayed candidates (Phone Link on Windows has no such mode) and holds it 50 s.

## Crate map

Each crate is one layer, or one feature, and names only the crates beneath it. Editing a feature rebuilds
that feature's crate and the ones above it, not the layers below.

| Crate (`crates/`) | Holds |
|---|---|
| `phonelink-proto` (`proto`) | the prost code generated from `proto/` |
| `phonelink-core` (`core`) | `types`, `time`, `sync`, `constants`, `net`, `tap`, `logging`, `config`, `codec`, `edge`, `identification`, `seams` |
| `phonelink-transport` (`transport`) | `auth`, `dcg`, `relay`, `platform` |
| `phonelink-app` (`app`), `phonelink-nano` (`nano`) | the App message layer; the Nano path |
| `phonelink-features` (`features`) | what the features share: `FeatureEnv`, `FeatureError`, the traits one feature implements for another, `shared/` |
| `phonelink-<feature>` (`<feature>`, with `-` for `_`) | one feature each; audio, Bluetooth, hotspot and screen mirroring also name the feature they compose (notifications, device status, Bluetooth, notifications) |
| `phonelink-engine` (this directory) | `engine`, `api`, `features::installed` (the installed set and the install order), the CLI, the golden checks and every integration test; it re-exports each crate beneath it at the module path the code and the tests use |

## Module map

| Module | Implements |
|---|---|
| `constants` | hosts, profiles, scope families, header names, API versions, lifetimes |
| `config` | every overridable default: timers, retries, feature switches, the identity the engine presents, endpoints |
| `identification` | the seven `DCG-*` client-identification headers |
| `types`, `seams` | the shared vocabulary and the traits between layers |
| `time` | conversions between `SystemTime`, Unix milliseconds and seconds, .NET ticks and `FILETIME`, and the civil calendar |
| `sync` | `MutexExt` and `RwLockExt`: every lock goes through `lock_or_panic` or `lock_or_recover`, never a bare `lock()` |
| `logging` | the app's log: size-bounded files (`store`), one line per event with each field public, scrubbed, or redacted to a tagged token (`format`, `policy`, `redact`), and the lines the app sends |
| `net` | the HTTP client every REST caller shares |
| `edge` | the OS-edge foreign traits (`SecureStore`, `SessionLock`, `HostInfo`, `Reachability`, `WebSignIn`), the stand-ins the CLI runs with (`standins`) and the in-memory implementations the tests use (`testing`) |
| `tap` | the decode seam: one `TapEvent` per message at every layer |
| `codec::hub` | SignalR MessagePack hub protocol v2 |
| `codec::json` | `OrderedStringMap`, a JSON object whose keys keep their order |
| `codec::dcg_packet`, `codec::envelope`, `codec::valueset` | `ms-dcg` packets, the binary inner envelope, `pbvalueset` |
| `proto` | prost code generated from `proto/` |
| `auth` | identity, enrollment, trust, wake |
| `auth::rest_policy` | the resilience policy of every DCG REST call: per service client a concurrency limit (1000) and a circuit breaker (10 % of at least 100 attempts in 30 s opens it for 5 s); per call a 30 s timeout, 3 retries of a transient attempt (status ≥ 500, 408, 429, no reply, attempt timeout) after a jittered exponential back-off from 2 s or the reply's `Retry-After`, a 10 s attempt timeout, and inside each attempt up to 3 resends of a 503, 504 or 429 with `Retry-Attempt`. Timers in `config::DcgRestConfig` |
| `relay` | the relay hub connection and partner presence |
| `dcg` | the fragment layer: `DcgIdentifiers` (the process's `MessageId` counter and per-target `SessionId`/`SequenceNumber`) and `HubOpener` (the open before each fragment try and ack) |
| `platform` | router, platform routes, per-partner connection |
| `app` | the App message layer |
| `nano` | the Nano path to the phone, below the features that use it (screen mirroring, files, camera): `contract` (offer and answer JSON), `ice` (candidates and the session description, STUN and TURN messages, the controlling agent), `srtp`, `transport` (MTU probe, keepalive, rate-control handshake, the rate-control header, receiver, sender and URCPV2 rate, the mux and its sequencer), `channel` (Message, Blob, video target, input source, audio target), `connection` (the connect object and the open connection over UDP), `connect` (attempt races, `/nanohandshake`, the context-message exchange), `cloud_relay`, `device` (the device connection and its platform channels), `named_streams`, `metrics`, `netif`, `probe` (the `PHONELINK_NANO_PROBE` diagnostic run) |
| `engine` | `EngineCore`: `wiring` (the identity stacks and the routes), the start sequence, `phones` (the selected phone), `cdeh` (the Cross Device identity's session), `install` (the features), `bridges` (device-list, account-removal and capability bridges), `system` (the Mac's lock / unlock / sleep / wake), and `events` (`CoreEvent` and the `CoreEvents` trait the `api` layer implements); the embedder's types (`FeatureEdges`, `StoreLocation`, `FetchMode`, `sign_out_stored`) |
| `api` | the `uniffi` surface, which only translates: `engine` (the `Engine` object: start, stop, refresh, foreground, system events, notification responses, the feature objects), `events` (`EngineEvent`, the `EventSink` foreign trait and the bridge from the core's events), the feature objects `gui/` drives (`NotificationsApi`, `AudioApi`, `ClipboardApi`, `DeviceStatusApi`, `RemoteControlApi`, `MessagingApi`, `ContactsApi`, `PhotosApi`, `WallpaperApi`, `RemoteSnippingApi`, `HotspotApi`, `PhoneApi`, …), the FFI records and enums mirroring the features' views, `FeatureApiError`, `BluetoothError`, `RequestOutcome`, and each feature's typed change event |
| `features` | the features; `shared/` holds what several of them use: `db` (one SQLite store per phone and feature), `notifier` (the `SystemNotifier` OS edge and the routing of responses), `peers` (the account's phones), `phone_switch` (the CDEH per-phone switch), `phone_number` (E.164 and the phone's country), `files_permission`, `launch_gates`, `nano_channels`; `installed` holds the installed set, `FeatureEdges` and the install order. Each feature's configuration is in `config::features` |
| `features::notifications` | the `notifications` content processor, `/notifications` and `/settings` requests and their triggers, the persistent action queue, connect provider #11, the banner decision |
| `features::audio` | media controls: the sessions made from the notification store's media-style items, the selection, the `/command/audio` request and its 20 s busy state, the per-phone audio-controls setting |
| `features::clipboard` | tag-9 announcements both ways, the `/clipboard` content pull and receiver, the feature-state sync; the `ClipboardPasteboard` and `ClipboardImageCodec` OS edges |
| `features::device_status` | the CDEH pull, push and `/remotedevicestatus` (OS edge `PcStatusSource`); the Phone Link connect flags, sync request, `deviceStatus` content and commands 2/3/9; the status per phone; the DND and ringer controls; ring my phone (`ring`: the ring control, its 20 s countdown and two-step stop, the `RING_MY_PHONE` request sent with `AppLayer::request_drm`); the low-battery notification. The Phone Link model is the phone record's `StatusKeys` |
| `features::messaging` | the `messaging` exchange, connect provider #10, `/chat/v1` requests, the messages prompt, new-message notifications; the `AttachmentImaging`, `MessagingFileTypes` and `PhoneProximity` OS edges |
| `features::contacts` | the `contactsV2` processor, `/contacts/v2` and the contacts `/permission`, connect provider #14, the lookup by address (`ContactDirectory`) |
| `features::photos` | the `mediaInfo` sync, connect provider #9, the thumbnail and full-resolution dispatchers, the media sync, delete and permission flows, cache maintenance, the new-photo notification (skipped for a phone `features::remote_snipping` covers) |
| `features::remote_control` | the `/remotecontrol` receiver and its gates, the `remoteControl` processor, the per-phone toggle, the `REMOTE_CONTROL`/`REMOTE_CONTROL_V2` capabilities; the `PcPowerControl` OS edge |
| `features::wallpaper` | the `wallpaper` content processor and connect provider #4; the Cross Device pull of the phone's wallpaper, the `/wallpaper` receiver (OS edge `PcWallpaperSource`, the size rule `wallpaper_scaled_size`) and the tag-5 subscription; both copies per phone and the one wallpaper shown |
| `features::bluetooth` | the phone's Bluetooth Classic device for every feature that reaches the phone over Bluetooth: connect provider #1 (the reply's Bluetooth keys into the phone record), which paired device is the phone, the pairing a feature runs when the user starts it and none is (the Bluetooth and `BluetoothAdvertise` `/permission` requests, the device-status sync for the Bluetooth name, discovery and pairing), one pairing at a time; the `PhoneBluetooth` OS edge |
| `features::hotspot` | instant hotspot: connect provider #2 (the reply's hotspot keys into the phone record), the connect — pairing through `features::bluetooth` and the phone's consent and permissions when missing (the consent notification request, `/hotspotdevicedatasyncrequest` the first time), then the RFCOMM side-channel credentials request (`wire`: varint32 frames, the data JWT, the reply's checks, RSA-OAEP credentials) and the join, whose SSID the record keeps; joined (the Mac's Wi-Fi on that SSID) and the disconnect that leaves it; the `HotspotWifi` OS edge |
| `features::file_transfer`, `features::phone_files`, `features::connected_camera`, `features::phone_apps`, `features::screen_mirroring` | file transfer, the phone's files, the phone as a camera, the phone's apps, and screen mirroring; each module's `//!` header says what it covers |
| `features::remote_snipping` | new-photo notifications on the CDEH identity: the MSAEP tag-6 and tag-3 subscribers, the `MEDIA` capability, thumbnail and photo requests, the notification and its activation, the `PERMISSIONS` request, the per-phone settings; the gate of photos' notification |

Each module's `//!` header says what it implements.

## Differences from Phone Link on Windows

What the engine does differently on purpose, so a later change does not "fix" it back. Each is also
marked where it is implemented ("differs from Phone Link on Windows").

| Area | The engine | Where |
|---|---|---|
| Sign-in | A device-bound Microsoft-account sign-in the engine runs itself, not through the OS's account manager: the Mac registers once as a Microsoft-account device, the user signs in once in a web view (`InlineLogin.srf`) for both identities, and each identity exchanges its own tokens under its own client id; there is no refresh token | `auth::live`, `auth::msa` |
| Identity | Its own values where Windows derives one from the OS: a random `DCG-LogicalDeviceId` minted once and stored; the display name is the host name cut before its first `.` | `config`, `identification` |
| Sign-out | Local only: deletes the stored identity and every feature store (Phone Link on Windows keeps the feature data); no unlink action | `auth`, `engine`, `features::db` |
| Connect message | PhoneApps is sent off, without sequence numbers; TelemetryInfo is omitted | `app::connect` |
| Malformed inbound content | Answered `{ result: 1 }` and dropped; Phone Link on Windows does not answer it | `codec::valueset`, `app` |
| Partner-disconnect breaker | `partner_disconnect_ack_timeouts` (5) consecutive `TimedOut` messages remove the partner | `dcg::sender` |
| Media controls | A stored item counts as updated only when its JSON text changed; the audio-controls setting is never turned off by a lost notification permission; no audio connection (the Mac is not made the phone's A2DP sink) | `features::audio` |
| Clipboard | The macOS pasteboard rules; a `/clipboard` request that fails the strict proto3 decode is answered `UNRECOGNIZED_PAYLOAD`; a failed image encode is answered `INVALID_CONTENT` | `features::clipboard` |
| Contacts | No contacts view: the composer's and Settings' prompts ask for contacts access; the birthday banner reads fewer date forms | `features::contacts`, `api::contacts` |
| Messaging | A send an earlier run left sending is marked failed and can be retried | `features::messaging` |
| Notifications | The action queue persists and is drained when its sender starts; triggers run whether or not a list is shown; a key of another JSON type reads as absent | `features::notifications` |
| Photos | The new-photo notification opens the image in the default macOS viewer; none is posted for a phone whose new photos remote snipping notifies (Phone Link on Windows can show both) | `features::photos` |
| Ring my phone | Every ring, and the ringing state a failed stop brings back, counts exactly 20 s; the control is a menu item and the stop confirmation an alert | `features::device_status` |
| Remote control | Update-and-shut-down / update-and-restart are the plain shutdown / restart, and the update query always answers no pending update; the per-phone toggle also gates `remoteControl` content (Phone Link on Windows has no switch there); a failed `remoteControl` command carries no `subError`; shutdown and restart are requested, not forced, so an application can still cancel them | `features::remote_control` |
| Wallpaper | `/wallpaper` answers the primary screen's desktop picture, never the solid-colour answer; its receiver is registered before the first pull rather than after it; one wallpaper per phone is shown from Phone Link's and the Cross Device copy | `features::wallpaper` |
| DCG REST resilience | A resend's `Retry-After` date is measured against UTC (Phone Link on Windows measures it against the PC's local clock); a `Retry-After` date in another form than RFC 1123 counts as none; a reply body must arrive within 100 s of its request (Phone Link on Windows sets no bound) | `auth::rest_policy`, `auth::dcg_rest` |
| Instant hotspot | The menu bar starts the connect and the engine joins the network itself (Phone Link on Windows puts an entry in the system Wi-Fi list); the hotspot reads as joined while the Mac's Wi-Fi is on the SSID the last connect joined, and disconnect leaves it; no background capability refresh; no separate setup — the connect pairs and asks for the phone's consent when either is missing; pairing matches the phone by name only; an inbound side-channel frame over 64 KiB is rejected; a failed join returns the credentials for a manual join | `features::hotspot` |
| Nano | A video Data packet outside its frame drops that frame (Phone Link on Windows terminates); a Nano channel of a type the PC does not wrap stays unclaimed (Phone Link on Windows fails on it); no Wi-Fi Direct attempt is offered | `nano::channel::video`, `nano::device`, `nano::connect` |
| Remote snipping | A clicked notification opens the image in the default macOS app for its type instead of Snipping Tool; a notification is withdrawn 5 minutes after it was posted; a publication from an unknown device or a `NEW_PHOTO` without an item or a last-modified time is dropped (Phone Link on Windows terminates) | `features::remote_snipping` |

## Local choices the protocol does not dictate

Design choices the protocol leaves open. They change no bytes on the wire, except in a failure such as
a store error; they are recorded so a later reader does not mistake them for protocol rules.

| Choice | Where |
|---|---|
| A `PartnerDisconnected` event that arrives after the partner is already back in the hub's partner set is ignored (no session end, no `SessionEvent`) | `platform::session` |
| A failure to obtain the MSA or services token before a due `SetDeviceInfo` (the periodic client-info refresh) records nothing (no `LastAttemptUpdateTime`), so the next start or options change tries again | `auth::identity` |
| A 2xx `EnrollDevice` whose body is empty or not JSON is a failure (`InvalidResponse`); a `{}` body is handled normally | `auth::identity` |
| A `GetAccountCert` failure fails the trust synchronization after the peer part was committed, so `EngineCore::start` stops with it | `auth::sync`, `engine` |
| Each key and its certificate are one `SecureStore` entry (`auth/<env>/identity-pair`, `auth/<env>/identity-pair-pending`, `auth/<env>/trust-pair`: JSON with the base64 PKCS#8 key and DER certificate), so they can never be stored half written. An entry that does not decode, whose key does not match its certificate, or whose CN is not the `dcgClientId` is treated as missing: the identity is cleared, or the trust certificate is sent again. A failing store read is an error, not "missing" | `auth::store`, `auth::identity` |
| Identity key rotation keeps the incoming pair as pending before `RotateKeys`: success promotes it, an answered failure drops it, a request without a reply keeps it. A `SignIn` answered `InvalidCertificate` while a pending pair exists is retried with that pair, which is promoted on success; a successful `SignIn` with the current key drops a pending pair | `auth::identity` |
| The strong-auth (`SAPI`) token of the `EnrollDevice` retry comes from a silent exchange for the `SAPI` scope; when the service refuses it the web sign-in runs once more, and a sign-in or consent page that signs in another account forgets the session and fails as needing the user | `auth::live::broker`, `auth::identity` |
| The hub's server timeout counts bytes read from the socket, beneath TLS, not completed messages, so a long inbound message keeps the connection alive; reading and writing run independently | `relay::connection` |
| A token failure while reachability reports offline opens as `InternetError`, whatever the token error | `relay::connection` |
| An open whose caller stops waiting is abandoned only while its start is in flight (the connection reports `Disconnected`); a start that finished meanwhile is kept. A close whose caller stops waiting runs to its end | `relay::connection`, `relay::manager` |
| The WebSocket client sets no message or frame size limit, so a fragment of up to the default 512 MiB fragment size is received whole | `relay::connection` |
| A wake cancelled because the hub connection reconnects has used its pending `NewSession` and ignore-device-disabled counts | `platform::session` |
| Attempts that resume or restart on their own carry reason `AutomaticReconnect` and a new root trace context; a retry after a back-off keeps the trace of the most recent request | `platform::session` |
| Every hub open runs in its own task, so a timeout or a stopped attempt never abandons an open midway | `platform::session`, `dcg` |
| Each feature keeps its state in its own SQLite database: `<data dir>/phones/<dcgClientId>.<Environment>/<feature>.sqlite3` per phone and `<data dir>/<feature>.sqlite3` for settings that belong to no phone; directories the engine creates are `0700` (an existing data directory keeps its mode), files `0600`; a database newer than the build's schema fails to open | `features::db` |
| A store failure while a feature handles inbound App content refuses the message like malformed content (`{result: 1}`, its transaction dropped, nothing applied; photos items and notification batches add `message`); one while a connect is built sends that provider's keys without the stored values (no sequence numbers or anchors) rather than invented ones — messaging computes its configuration as for a phone that has reported no capabilities when its stored capabilities cannot be read | the features with an App-content store: notifications, messaging, contacts, photos |
| A `DevicePermissions` flag no writer has reported reads its feature's default (`PhoneRecord::permission_or`); a report that leaves the bit unchanged but makes it known emits `PermissionsChanged` | `app::phone_record`, `app` |
| The features are installed once per `EngineCore`, before the App layer starts (`Drm` has no unregister, so the clipboard and device-status receivers stay registered). An install that fails part-way leaves the core needing a rebuild (`EngineCore::needs_rebuild`); the `uniffi` `Engine` then builds a new core on its next `start` | `engine`, `features::installed` |
| A panic while the App layer processes one inbound message is isolated to it: logged, its transaction discarded, an answerable `/legacy/phonecontent` request refused `{ result: 1 }`; the worker goes on. A store lock poisoned by a panic is recovered (the open transaction was rolled back), and so are the App layer's own locks; a phone-record update is applied to a copy, so a panicking update leaves the record unchanged | `app::inbound`, `app`, `features::db` |
| `EngineCore::stop` cancels in-flight connects (as the disconnect trigger does) before it ends the session, and a later `start` sends the background connect of an app start; synchronous entry points the foreign side calls on its own threads (`notification_response`, `refresh`, `set_foreground`) run inside the engine's runtime | `engine` |
| The app's foreground state comes from `Engine::set_foreground` (macOS: the application became or resigned active). Reported before or while a core starts, it is recorded and the App layer's start sends the app-start connect, then `AppActivated` when the app is in the foreground; reported later, coming to the foreground sends `AppActivated` at once; reporting the same state again sends nothing | `engine`, `app` |
| An account deprovisioned while the features run: in-flight connects are cancelled, the clipboard shuts down, the capability bridge stops, and the core needs a rebuild (`EngineCore::start` refuses); the `uniffi` `Engine`'s next `start` stops that core and builds a new one | `engine` |
| The account gate's "token cannot be refreshed" is set by any MSA token request or sign-in that needs the user or that the user declined, for the DCG and the strong-auth (`SAPI`) scope alike, and by a sign-out; the next one that succeeds clears it | `auth::msa` |
| The CLI answers the phone's `/remotedevicestatus` `UNKNOWN_ERROR` (it cannot read the host's power and network state) rather than inventing values; its other feature edges are the in-memory stand-ins; the PC's region is the region of the POSIX locale (`LC_ALL`, else `LANG`: `ko_KR.UTF-8` → `KR`) | `bin/phonelink` |
| The Phone Link device-status model lives in the phone record (`StatusKeys`, persisted with it; `Some` marks a key the phone delivered); its change counters are not persisted | `app::phone_record`, `features::device_status` |
| Notifications: clear-all removes its rows right after the operation-4 outcome, then sends the silent check; a phone's action-queue sender sends only while its phone is selected; `press_action` and `reply` refuse a key that is not stored (`UnknownNotification`) or an action index its item does not offer (`UnknownAction`), `set_pinned` a key that is not stored, and `request_sensitive_access` while the prompt is not offered (`NotAvailable`); `dismiss` sends operation 2 for any key | `features::notifications` |
| Clipboard: the first change-count read is the baseline; overlapping announcements read, hash and dedupe one at a time; a change of the global switch re-subscribes tag 9 (forgetting the last delivered `message_id`); after `shutdown()` the receiver stays registered and answers nothing; a store failure answers nothing on `/clipboard` (the phone's request times out), drops an inbound announcement, and sends no announcement, no content pull and no feature state (`clipboardFeatureStatusUpToDate` is left) | `features::clipboard` |
| Ring my phone: a failed stop brings the ringing state back only while the control still shows off (a ring started meanwhile, or a phone that went away, is left as it is); a ring of a phone that is no longer selected is not shown and ends with its countdown | `features::device_status` |
| Device status: `/remotedevicestatus`'s battery percent is current × 100 ⁄ max rounded down, capped at 100, and a battery whose maximum is ≤ 0 counts as none; a peer removed from the device list drops its CDEH model | `features::device_status` |
| Messaging: a pending send keeps its text, recipients, SIM and fitted parts so a retry of a failed send works after a restart; the parts of an MMS that a metadata delta or an item's `Delete` row deletes are deleted too; a failed image fit sends the original; a send over the carrier's text or recipient limit is refused, and `can_reply` is false for a conversation over the recipient limit; a pending send still marked sending when the phone's store is first opened in a run is marked failed (`Unknown`), offering the retry; a second download of a part in flight is refused, and `download_part` returns the request's outcome; the mark-read triggers that need the window focused read the focus the UI reports (`window_focused`); views leave part bytes in the store (`part_bytes` reads one); attachment imaging runs on the blocking pool; a message notification's user info carries `thread` (the conversation to open, absent for a send without one) and, with the reply action, `replyThread` and `replyRcs` — a reply without `replyThread` is dropped with an error; an RCS notification reply's pending send has its own origin (code 3), which lists it under its RCS conversation's fallback id | `features::messaging` |
| Contacts: the lookup index is cached per phone and country and dropped after each committed change; a lookup reads only the candidates' names and whether a thumbnail is stored, and names the contact whose photo to show (the first with one); the list is ordered by display name (ASCII case-insensitive, SQLite `NOCASE`), then id; clearing the records keeps PC consent, "set up once" and the last-updated time; a store failure outside inbound content and the connect build is logged and goes no further — a lookup finds no contact, a connect response's expected sequence numbers are not stored (its `permissions.contacts` still applies), and a success or accepted outcome grants the contacts permission without storing "contacts set up once" | `features::contacts` |
| Wallpaper: a connect's gate is remembered from its build to its reply by `correlationVector` (the last 16 connects); without one the reply is gated on the record after DeviceInfo applied it; the Cross Device pulls of one phone may overlap; the edge's bytes must start like a JPEG, else the answer is the unreadable one; an edge read that fails is answered as no picture | `features::wallpaper` |
| Photos: display-order ties are broken by id, descending; requests for a phone that is no longer selected are not sent and its queues are cleared; hidden-after-delete ids, delete states and the last pop-up time live in memory; a delete reply handled while the store cannot be read still ends the delete (nothing leaves the gallery, the item is no longer "deleting", `DeleteFinished` is sent) | `features::photos` |
| The `uniffi` objects act on the phone the start selected (the UI shows one phone); each call resolves the engine's current core, so an object taken before a start, or before a rebuild, reaches the core that runs; a feature error maps to `FeatureApiError` (`NotStarted` also for an App layer that is gone) and an `AppOutcome` to `RequestOutcome` with a failed connection's or send's cause kept as text, except *LTW disabled* (→ `AllFeaturesDisabled`) and a session ended `METERED_SYNC_DISABLED` (→ `MeteredSyncDisabled`) | `api` |
| The features' events reach the UI through the event sink as `EngineEvent`s of their feature; a lagged event stream sends `Resync` for that feature instead of the lost events. Session events are `PhoneSession`, the removal of the selected phone `PhoneUnlinked` (once; App requests then fail at the account gate until a start selects a phone again), the account's removal `AccountRemoved`, a changed phone record or permission set `PhoneChanged` | `engine`, `api` |
| The conversation list previews each conversation's latest inbox or sent message — the one its `latest_ms` comes from, mass-text copies and RCS group events left out, ties to the higher id — as the conversation view shows it (an MMS's text parts as its body, any other part as an attachment) | `features::messaging` |
| Contact suggestions for a recipient: the numbers of contacts whose name contains the query (case-insensitively) or whose number contains its digits, in the contact list's order; offered only while contacts access is granted, from one read of the stored numbers (no thumbnails) | `api::contacts` |
| Remote control: the Cross Device receiver serves the selected phone only, as the other Cross Device features; a store failure reading the toggle or the phone's switch answers `FAILED` (and refuses `remoteControl` content like malformed content); a phone the device-list refresh brings in is checked against the refreshed list at once | `features::remote_control` |
| Remote snipping: the per-phone state is one row of the phone's `remote_snipping` store, with the phone's last capability answer (none, capable, not capable); the rate limit, the set of phones whose new-photo publication was handled, and the thumbnails of shown notifications live in memory; a store or switch failure counts the phone as covered (no photos notification) and drops a publication; the trace state of each request carries its scenario and trigger names | `features::remote_snipping` |
| `Environment` is not exported: no exported call uses it, and its Swift name would shadow SwiftUI's `Environment` | `types` |
| The phone's TURN `credential` from CloudRelay is redacted in tap events like a bearer token | `auth::dcg_rest` |
| Nano device connection: one device connection per phone is kept while something holds it | `nano::device` |
