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
