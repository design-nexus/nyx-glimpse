# Glimpse panels

Mini-app panels for Nyx apps, drawn in Glimpse's preview area when the app's result is
selected (`music`, `music jazz`). Only the panels in this folder run in the shell; an
app's manifest picks one by name (`nyx.panel.id`), and the backend offers only the IDs
in `commands::PANELS` (`example` too for a test copy started with
`GLIMPSE_TEST_PANELS=1`).

A panel is `panels/<id>/Panel.qml`, a `FocusScope` with `property var host`. It talks
to the app only through the host (`PanelHost.qml`): `host.query`, `host.topic(name)`,
`host.run(action, arguments, done)`, `host.open(arguments)`, `host.say(text)` and
`host.back()`. It is created when its result is selected and destroyed when it isn't,
which ends its topics; it draws with `Nyx.Ui` only.

In Glimpse a panel has the dropdown's slot: the full width across the top, 728 by about
230, the same for every panel and never larger (`host.slot` is `"dropdown"`, and
`host.compact` is true). Lay it out wide and short there: one row of what matters most,
with lists as a strip beside it. Pinned, it has its own surface at the size its app asks
for (`nyx.panel.size`, `min` and `preferred`), and `host.slot` is `"pinned"`.

Ctrl+P pins the shown panel (and unpins it): it stays on screen after Glimpse closes, in
`Pins.qml`, as a persistent overlay Shell Core pins and places (`PinOverlay`; Nyx.Shell's
standalone mode on Omarchy). The backend keeps the pins in Glimpse's settings
(`pinned`, at most six) and publishes them on `pins`; a pinned panel is the same QML
with the same host, and follows its topics only while it shows.
