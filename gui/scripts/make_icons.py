#!/usr/bin/env python3
"""Generate the app's icons from one geometry: the Icon Composer app icon, the menu bar template
glyph, and the flat master artwork.

    just icons          (from the repository root; runs `python3 -I gui/scripts/make_icons.py`)

Writes (all SVG, so every icon stays editable as vector):
- PhoneLink/App/AppIcon.icon/            the app icon (Icon Composer document; layers in Assets/)
- PhoneLink/Assets.xcassets/MenuBarIcon  the menu bar extra's template image
- Design/AppIcon.svg                     both layers on the white tile, flat (no system glass)

The mark: a Mac display on its stand, centred on the icon grid's vertical axis, with an Android phone
(punch-hole camera) standing in front of its lower-right corner — lower than the display, so it reads
as nearer. Canvas: the 1024pt full-bleed icon on Icon Composer's grid (128pt cells, outer keyline
128..896, centre 512,512); macOS applies its own rounded-rect mask.

The layers are flat fills. The system supplies the Liquid Glass: each group is glass with
translucency, so the display shows through the phone, and casts a shadow in its own colour.
"""
import json
import pathlib

GUI = pathlib.Path(__file__).resolve().parent.parent

DISPLAY_FILL = "#2F7BFF"
PHONE_FILL = "#56D0FF"
CAMERA_FILL = "#FFFFFF"


def rr(x, y, w, h, r):
    return (f"M{x+r:.2f},{y:.2f} H{x+w-r:.2f} A{r:.2f},{r:.2f} 0 0 1 {x+w:.2f},{y+r:.2f} V{y+h-r:.2f} "
            f"A{r:.2f},{r:.2f} 0 0 1 {x+w-r:.2f},{y+h:.2f} H{x+r:.2f} A{r:.2f},{r:.2f} 0 0 1 {x:.2f},{y+h-r:.2f} "
            f"V{y+r:.2f} A{r:.2f},{r:.2f} 0 0 1 {x+r:.2f},{y:.2f} Z")


def svg(w, h, body, defs=""):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:g}" height="{h:g}" viewBox="0 0 {w:g} {h:g}">'
            + (f"<defs>{defs}</defs>" if defs else "") + f"{body}</svg>\n")


# ---- geometry (1024 canvas) -----------------------------------------------------------------------
DW, DH, DR = 640, 400, 54               # display, 16:10, centred on x = 512 (192..832)
NECK_H, FOOT_W, FOOT_H = 84, 236, 34
PW, PH, PR = 206, 420, 50               # phone, ~1:2
OVERHANG = 64                           # phone past the display's right edge, to the 896 keyline
HOLE_R, HOLE_Y = 9, 36                  # punch-hole: centred, near the phone's top edge
PHONE_TOP = 403                         # phone 403..823: its bottom 44 below the stand's foot
DISPLAY_TOP = 261                       # display 261..661, top a third of a cell above the phone's

GROW = 1.08                             # every element scaled about its own centre
STRETCH_Y = 1.06                        # the two screens only (not the stand): taller, same centre

SHIFT_X = -11.75                        # optical: half the mark's rightward lean (the phone side)
DX = 512 - DW / 2 + SHIFT_X
FOOT_Y = DISPLAY_TOP + DH + NECK_H
CX = DX + DW / 2
QX = DX + DW + OVERHANG - PW
QY = PHONE_TOP


def grown(x, y, w, h, r, stretch_y=1.0):
    """The box scaled by GROW about its centre (radius too), its height by stretch_y on top."""
    cx, cy = x + w / 2, y + h / 2
    gw, gh = w * GROW, h * GROW * stretch_y
    return cx - gw / 2, cy - gh / 2, gw, gh, r * GROW


def grown_points(points):
    cx = sum(p[0] for p in points) / len(points)
    cy = sum(p[1] for p in points) / len(points)
    return [(cx + (x - cx) * GROW, cy + (y - cy) * GROW) for x, y in points]


NECK_PTS = grown_points([(CX - 40, DISPLAY_TOP + DH - 8), (CX + 40, DISPLAY_TOP + DH - 8),
                         (CX + 54, FOOT_Y + 4), (CX - 54, FOOT_Y + 4)])
PHONE_BOX = grown(QX, QY, PW, PH, PR, STRETCH_Y)

PANEL_D = rr(*grown(DX, DISPLAY_TOP, DW, DH, DR, STRETCH_Y))
NECK_D = "M" + " L".join(f"{x:.2f},{y:.2f}" for x, y in NECK_PTS) + " Z"
FOOT_D = rr(*grown(CX - FOOT_W / 2, FOOT_Y, FOOT_W, FOOT_H, FOOT_H / 2))
PHONE_D = rr(*PHONE_BOX)
CAM = (QX + PW / 2, PHONE_BOX[1] + HOLE_Y * GROW, HOLE_R * GROW)     # same distance below the top edge

DISPLAY_BODY = (f'<path d="{NECK_D}" fill="{DISPLAY_FILL}"/><path d="{FOOT_D}" fill="{DISPLAY_FILL}"/>'
                f'<path d="{PANEL_D}" fill="{DISPLAY_FILL}"/>')
PHONE_BODY = (f'<path d="{PHONE_D}" fill="{PHONE_FILL}"/>'
              f'<circle cx="{CAM[0]:.2f}" cy="{CAM[1]:.2f}" r="{CAM[2]}" fill="{CAMERA_FILL}"/>')


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


# ---- app icon: Icon Composer document ------------------------------------------------------------
icon = GUI / "PhoneLink/App/AppIcon.icon"
write(icon / "Assets/display.svg", svg(1024, 1024, DISPLAY_BODY))
write(icon / "Assets/phone.svg", svg(1024, 1024, PHONE_BODY))


def group(name, translucency):
    return {"layers": [{"glass": True, "image-name": f"{name}.svg", "name": name}],
            "shadow": {"kind": "layer-color", "opacity": 0.5},
            "specular": True,
            "translucency": {"enabled": True, "value": translucency}}


write(icon / "icon.json", json.dumps({
    "fill": {"solid": "extended-srgb:1.00000,1.00000,1.00000,1.00000"},
    "groups": [group("phone", 0.5), group("display", 0.45)],
    "supported-platforms": {"squares": ["macOS"]},
}, indent=2) + "\n")

# ---- flat master: both layers on the white tile -------------------------------------------------
write(GUI / "Design/AppIcon.svg", svg(1024, 1024,
    '<rect width="1024" height="1024" fill="#FFFFFF"/>' + DISPLAY_BODY + PHONE_BODY))

# ---- menu bar glyph: template image, 18pt high --------------------------------------------------
# The display is solid with the phone knocked out of it by a gap; the phone is an outline, so the two
# devices stay apart at menu bar size. Black + alpha only: the system tints a template image.
TOP, BOTTOM, RIGHT = DISPLAY_TOP, QY + PH, QX + PW
S = 18 / (BOTTOM - TOP)
W = (RIGHT - DX) * S
GAP, STROKE = 1.5, 1.4
mx, my = (lambda v: (v - DX) * S), (lambda v: (v - TOP) * S)
phone_box = (mx(QX), my(QY), PW * S, PH * S, PR * S)
menu = svg(W, 18,
    '<g mask="url(#gap)" fill="#000">'
    f'<path d="{rr(0, 0, DW*S, DH*S, DR*S)}"/>'
    f'<path d="M{mx(CX-40):.2f},{my(DISPLAY_TOP+DH-2):.2f} H{mx(CX+40):.2f} L{mx(CX+54):.2f},{my(FOOT_Y+4):.2f} H{mx(CX-54):.2f} Z"/>'
    f'<path d="{rr(mx(CX - FOOT_W/2), my(FOOT_Y), FOOT_W*S, FOOT_H*S, FOOT_H*S/2)}"/>'
    '</g>'
    f'<path d="{rr(*phone_box)} {rr(phone_box[0] + STROKE, phone_box[1] + STROKE, phone_box[2] - 2*STROKE, phone_box[3] - 2*STROKE, phone_box[4] - STROKE)}" '
    'fill="#000" fill-rule="evenodd"/>',
    f'<mask id="gap" maskUnits="userSpaceOnUse" x="0" y="0" width="{W:.2f}" height="18">'
    f'<rect width="{W:.2f}" height="18" fill="#fff"/>'
    f'<path d="{rr(phone_box[0] - GAP, phone_box[1] - GAP, phone_box[2] + 2*GAP, phone_box[3] + 2*GAP, phone_box[4] + GAP)}" fill="#000"/>'
    '</mask>')
assets = GUI / "PhoneLink/Assets.xcassets"
write(assets / "Contents.json", json.dumps({"info": {"author": "xcode", "version": 1}}, indent=2) + "\n")
write(assets / "MenuBarIcon.imageset/MenuBarIcon.svg", menu)
write(assets / "MenuBarIcon.imageset/Contents.json", json.dumps({
    "images": [{"filename": "MenuBarIcon.svg", "idiom": "universal"}],
    "info": {"author": "xcode", "version": 1},
    "properties": {"preserves-vector-representation": True, "template-rendering-intent": "template"},
}, indent=2) + "\n")

print(f"wrote {icon.relative_to(GUI)}, {assets.relative_to(GUI)}/MenuBarIcon.imageset, Design/AppIcon.svg")
