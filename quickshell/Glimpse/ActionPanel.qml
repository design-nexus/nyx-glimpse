import QtQuick
import QtQuick.Effects
import Nyx.Ui

// The action panel (Ctrl+K) under the search bar: a searchable list of what can be done
// to the selected result. Set alias… turns the field into the alias; View details shows
// the result's title, second line and id. Up on an empty search opens the search
// history here instead. Escape goes back to the same search and selection.
Item {
    id: ap

    required property var glimpse
    required property var backend
    property bool open: false
    // actions, alias, details or history.
    property string mode: "actions"
    property var items: []
    property string rowId: ""
    property string alias: ""
    property string details: ""
    property int current: 0
    property int request: 0
    readonly property var shown: {
        if (mode === "alias")
            return [{ title: field.text.trim() === "" ? "Remove alias" : "Save alias: " + field.text, verb: "save-alias" }];
        const q = field.text.toLowerCase();
        return items.filter(i => q === "" || i.title.toLowerCase().indexOf(q) >= 0);
    }

    signal closed
    signal picked(string query)

    visible: open
    width: 330
    height: Math.min(box.implicitHeight, 300 + 56)

    // "<Control>k" as "Ctrl+K".
    function caption(spec) {
        if (!spec)
            return "";
        const names = { control: "Ctrl", ctrl: "Ctrl", primary: "Ctrl", shift: "Shift", alt: "Alt", mod1: "Alt", super: "Super", meta: "Super" };
        let out = [];
        let rest = spec, m;
        while ((m = rest.match(/^<([^>]+)>/))) {
            out.push(names[m[1].toLowerCase()] || m[1]);
            rest = rest.substring(m[0].length);
        }
        out.push(rest.length === 1 ? rest.toUpperCase() : rest);
        return out.join("+");
    }
    function show(m) {
        mode = m;
        field.text = "";
        current = 0;
        open = true;
        field.forceActiveFocus();
    }
    function close() {
        if (!open)
            return;
        open = false;
        request += 1;
        closed();
    }
    function openFor(row) {
        rowId = row.id;
        items = [];
        details = "";
        const mine = ++request;
        backend.request("Actions", { id: row.id, generation: glimpse.generation }, (ok, err) => {
            if (mine !== ap.request || err || !ok)
                return;
            ap.items = ok.items;
            ap.alias = ok.alias || "";
            ap.details = ok.details || "";
            if (row.kind === "plugin")
                backend.request("ContextActions", { id: row.id, generation: glimpse.generation }, (more, e2) => {
                    if (mine === ap.request && more && more.items.length > 0)
                        ap.items = ap.items.concat(more.items);
                });
        });
        show("actions");
    }
    function openHistory() {
        const mine = ++request;
        backend.request("History", {}, (ok, err) => {
            if (mine !== ap.request || err || !ok || ok.items.length === 0)
                return;
            ap.items = ok.items.map(q => ({ title: q, verb: "history" }));
            ap.show("history");
        });
    }
    function run(index) {
        const item = shown[index];
        if (!item)
            return;
        if (item.verb === "history") {
            close();
            picked(item.title);
            return;
        }
        if (item.verb === "alias") {
            mode = "alias";
            field.text = alias;
            field.cursorPosition = alias.length;
            current = 0;
            return;
        }
        if (item.verb === "details") {
            field.text = "";
            mode = "details";
            return;
        }
        const at = glimpse.rows.findIndex(r => r.id === rowId);
        const how = item.verb === "save-alias" ? "alias:" + field.text.trim() : item.verb === "open" ? "enter" : item.verb;
        close();
        if (at >= 0)
            glimpse.activate(at, how);
    }

    MultiEffect {
        anchors.fill: bg
        source: bg
        shadowEnabled: true
        shadowColor: Qt.rgba(0, 0, 0, 0.35)
        shadowBlur: 0.8
        shadowVerticalOffset: 6
        blurMax: 24
    }
    Rectangle {
        id: bg
        anchors.fill: parent
        radius: Style.radiusCard
        color: Style.popover
        border.width: 1
        border.color: Style.alpha(Style.text, 0.08)
    }
    MouseArea {
        anchors.fill: parent
    }

    Column {
        id: box
        width: parent.width
        padding: 6
        spacing: 6

        Rectangle {
            width: parent.width - 12
            height: Style.controlHeight + 2
            radius: Style.radiusSmall
            color: Style.field
            border.width: field.activeFocus ? 2 : 1
            border.color: field.activeFocus ? Style.accent : Style.alpha(Style.border, 0.5)
            Icon {
                id: findIcon
                anchors.left: parent.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                name: "edit-find-symbolic"
                size: 16
                color: Style.tertiary
            }
            TextInput {
                id: field
                anchors.left: findIcon.right
                anchors.leftMargin: 8
                anchors.right: parent.right
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                color: Style.text
                selectionColor: Style.alpha(Style.accent, 0.4)
                font.family: Style.font
                font.pixelSize: 13
                clip: true
                onTextChanged: ap.current = 0
                Keys.priority: Keys.BeforeItem
                Keys.onPressed: e => {
                    const n = ap.shown.length;
                    if (e.key === Qt.Key_Escape) {
                        ap.close();
                    } else if ((e.key === Qt.Key_Up || e.key === Qt.Key_Down) && n > 0 && ap.mode !== "details") {
                        ap.current = (ap.current + (e.key === Qt.Key_Down ? 1 : -1) + n) % n;
                    } else if ((e.key === Qt.Key_Return || e.key === Qt.Key_Enter) && ap.mode !== "details") {
                        ap.run(ap.current);
                    } else {
                        return;
                    }
                    e.accepted = true;
                }
                Text {
                    anchors.fill: parent
                    verticalAlignment: Text.AlignVCenter
                    visible: field.text === ""
                    text: ap.mode === "alias" ? "Alias (empty removes it)" : ap.mode === "history" ? "Search history…" : "Search actions…"
                    color: Style.tertiary
                    font: field.font
                }
            }
        }

        TextEdit {
            visible: ap.mode === "details"
            width: parent.width - 12
            padding: 10
            text: ap.details
            readOnly: true
            selectByMouse: true
            wrapMode: TextEdit.WrapAtWordBoundaryOrAnywhere
            color: Style.text
            selectionColor: Style.alpha(Style.accent, 0.4)
            font.family: Style.font
            font.pointSize: 12.5 * 0.75
        }

        ListView {
            id: list
            visible: ap.mode !== "details"
            width: parent.width - 12
            height: Math.min(contentHeight, 300)
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: ap.shown
            currentIndex: ap.current
            onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)
            delegate: Rectangle {
                required property var modelData
                required property int index
                width: list.width
                height: 34
                radius: Style.radiusSmall
                color: index === ap.current ? Style.selected : rowArea.containsMouse ? Style.rowHover : "transparent"
                Text {
                    anchors.left: parent.left
                    anchors.leftMargin: 10
                    anchors.right: parent.right
                    anchors.rightMargin: 10
                    anchors.verticalCenter: parent.verticalCenter
                    text: parent.modelData.title
                    elide: Text.ElideRight
                    color: Style.text
                    font.family: Style.font
                    font.pointSize: 12.5 * 0.75
                }
                MouseArea {
                    id: rowArea
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: ap.run(parent.index)
                }
            }
        }
    }
}
