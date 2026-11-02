#!/usr/bin/env python3
"""Run the one native action-panel regression on an isolated Wayland output.
No models, real indexed sources, accessibility bus, or global shortcuts are used.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
base = Path(tempfile.mkdtemp(prefix="glimpse-ui-"))
monitor = f"HEADLESS-GLIMPSE-{os.getpid()}"
env = os.environ.copy()
for key, folder in [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                    ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime")]:
    path = base / folder
    path.mkdir(mode=0o700)
    env[key] = str(path)
env["WAYLAND_DISPLAY"] = str(Path(os.environ["XDG_RUNTIME_DIR"]) / os.environ["WAYLAND_DISPLAY"])
env.update(GLIMPSE_MONITOR=monitor, GLIMPSE_TRACE="1", GTK_A11Y="none",
           GLIMPSE_TEST_SCREENSHOT=str(base / "actions.png"))
applications = base / "data/applications"
applications.mkdir()
(applications / "glimpse-audit.desktop").write_text(
    "[Desktop Entry]\nType=Application\nName=Glimpseaudit\nExec=/usr/bin/true\n"
    "Icon=system-search\nActions=NewWindow;\n[Desktop Action NewWindow]\n"
    "Name=New Window\nExec=/usr/bin/true\n")
config = base / "config/nyx-glimpse"
config.mkdir()
(config / "settings.toml").write_text(
    'learning = false\nsemantic = false\nmode = "theme"\ntheme = "nyx-dark"\n')
print(f"UI artifacts: {base}", flush=True)
subprocess.run(["hyprctl", "output", "create", "headless", monitor], check=True, stdout=subprocess.DEVNULL)
try:
    with (base / "workflow.log").open("w") as log:
        result = subprocess.run(
            ["cargo", "test", "--release", "--locked", "-p", "nyx-glimpse", "--bin", "nyx-glimpse",
             "action_panel_alias_and_escape_workflow", "--", "--ignored", "--test-threads=1", "--nocapture"],
            cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=120)
    text = (base / "workflow.log").read_text()
    print(text[-5000:])
    timings = [json.loads(line.split("glimpse-metric ", 1)[1])
               for line in text.splitlines() if "glimpse-metric " in line]
    (base / "timings.json").write_text(json.dumps({"kind": "native GTK workflow; synthetic text changes, not physical hotkeys",
                                                 "timings": timings}, indent=2) + "\n")
    raise SystemExit(result.returncode)
finally:
    subprocess.run(["hyprctl", "output", "remove", monitor], check=False, stdout=subprocess.DEVNULL)
