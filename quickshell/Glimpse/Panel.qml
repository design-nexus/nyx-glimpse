import QtQuick
import QtQuick.Effects
import Nyx.Ui

// The launcher panel (the GTK panel's `glimpse-frame`): 760 wide, 56 tall with an empty
// search, 460 once there is something under it.
Item {
    id: panel

    required property var glimpse
    required property var backend
    readonly property alias text: input.text
    readonly property bool expanded: glimpse.expanded
    property int maxWidth: 10000
    property int maxHeight: 10000
    // A selected panel's preferred size widens and heightens the frame, within the screen.
    readonly property var miniSize: glimpse.row && glimpse.row.kind === "panel" && glimpse.row.panel ? glimpse.row.panel.size : null
    readonly property var preferred: miniSize && miniSize.preferred ? miniSize.preferred : null
    readonly property int expandedHeight: Math.min(maxHeight, Math.max(460, preferred ? preferred[1] + 54 + 36 + 18 : 0))

    width: Math.min(maxWidth, Math.max(760, preferred ? 300 + preferred[0] + 32 : 0))
    height: expanded ? expandedHeight : 56

    function setText(t) {
        input.text = t;
        input.cursorPosition = t.length;
        if (t === "")
            glimpse.search("");
    }
    function focusSearch() {
        input.forceActiveFocus();
    }
    function scrollTop() {
        list.positionViewAtBeginning();
    }
    function closeActions() {
        actions.close();
    }
    function openActions(row) {
        actions.openFor(row);
    }
    function openHistory() {
        actions.openHistory();
    }
    function miniApp(fn) {
        preview.panelCall(fn);
    }
    function focusMiniApp() {
        preview.focusPanel();
    }

    // A GTK accelerator ("<Control><Shift>f") against a key press.
    function accel(spec, e) {
        if (!spec)
            return false;
        let mods = 0;
        let rest = spec;
        let m;
        while ((m = rest.match(/^<([^>]+)>/))) {
            const t = m[1].toLowerCase();
            if (t === "control" || t === "ctrl" || t === "primary")
                mods |= Qt.ControlModifier;
            else if (t === "shift")
                mods |= Qt.ShiftModifier;
            else if (t === "alt" || t === "mod1")
                mods |= Qt.AltModifier;
            else if (t === "super" || t === "meta" || t === "mod4")
                mods |= Qt.MetaModifier;
            rest = rest.substring(m[0].length);
        }
        if ((e.modifiers & (Qt.ControlModifier | Qt.ShiftModifier | Qt.AltModifier | Qt.MetaModifier)) !== mods)
            return false;
        const k = rest.toLowerCase();
        const named = { space: Qt.Key_Space, "return": Qt.Key_Return, comma: Qt.Key_Comma, period: Qt.Key_Period, slash: Qt.Key_Slash, semicolon: Qt.Key_Semicolon, tab: Qt.Key_Tab };
        if (named[k] !== undefined)
            return e.key === named[k];
        if (rest.length === 1)
            return e.key === rest.toUpperCase().charCodeAt(0);
        if (/^f\d+$/.test(k))
            return e.key === Qt.Key_F1 + parseInt(k.substring(1)) - 1;
        return false;
    }

    function keyPressed(e) {
        const g = glimpse;
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        const alt = (e.modifiers & Qt.AltModifier) !== 0;
        if (accel(g.config.action_key, e)) {
            if (g.row)
                actions.openFor(g.row);
        } else if (accel(g.config.favorite_key, e)) {
            if (g.row)
                g.activate(g.selected, "favorite");
        } else if (e.key === Qt.Key_Up && input.text === "") {
            actions.openHistory();
        } else if (e.key === Qt.Key_Escape) {
            g.hide();
        } else if (e.key === Qt.Key_Down || e.key === Qt.Key_Up || e.key === Qt.Key_PageDown || e.key === Qt.Key_PageUp) {
            const step = ({ [Qt.Key_Down]: 1, [Qt.Key_Up]: -1, [Qt.Key_PageDown]: 8, [Qt.Key_PageUp]: -8 })[e.key];
            g.select(g.selected + step);
        } else if (ctrl && (e.key === Qt.Key_Home || e.key === Qt.Key_End)) {
            // Plain Home and End stay with the search text.
            g.select(e.key === Qt.Key_Home ? 0 : g.rows.length - 1);
        } else if (ctrl && e.key === Qt.Key_G) {
            g.select(g.selected + ((e.modifiers & Qt.ShiftModifier) ? -1 : 1));
        } else if (ctrl && e.key === Qt.Key_P && preview.playing !== undefined && preview.hasMedia) {
            preview.togglePlayback();
        } else if (e.key === Qt.Key_Tab || e.key === Qt.Key_Backtab) {
            // A command's completion; the keyboard stays in the search either way.
            if (g.row && g.row.fill)
                setText(g.row.fill);
        } else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            g.activate(g.selected, ctrl ? "reveal" : alt ? "alt" : "enter");
        } else if (ctrl && e.key === Qt.Key_O && g.row && g.row.kind === "password") {
            g.activate(g.selected, "site");
        } else if (ctrl && e.key === Qt.Key_C && input.selectedText === "" && g.row) {
            // With text selected in the search, Ctrl+C copies that instead.
            g.activate(g.selected, "copy");
        } else {
            return;
        }
        e.accepted = true;
    }

    MultiEffect {
        anchors.fill: frame
        source: frame
        shadowEnabled: true
        shadowColor: Qt.rgba(0, 0, 0, 0.38)
        shadowBlur: 1.0
        shadowVerticalOffset: 12
        blurMax: 40
    }

    Rectangle {
        id: frame
        anchors.fill: parent
        radius: Style.radiusDialog
        color: Style.bg
        border.width: 1
        border.color: Style.alpha(Style.text, 0.06)
        clip: true

        // A click on the panel stays on the panel, and closes the action panel.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
            onPressed: actions.close()
        }

        Item {
            id: bar
            width: parent.width
            height: 54

            Icon {
                id: searchIcon
                anchors.left: parent.left
                anchors.leftMargin: 16
                anchors.verticalCenter: parent.verticalCenter
                name: "edit-find-symbolic"
                size: 16
                color: Style.dimText
            }
            TextInput {
                id: input
                anchors.left: searchIcon.right
                anchors.leftMargin: 10
                anchors.right: parent.right
                anchors.rightMargin: 16
                anchors.verticalCenter: parent.verticalCenter
                color: Style.text
                selectionColor: Style.alpha(Style.accent, 0.4)
                selectedTextColor: Style.text
                font.family: Style.font
                font.pixelSize: 15
                clip: true
                focus: true
                Keys.priority: Keys.BeforeItem
                Keys.onPressed: e => panel.keyPressed(e)
                onTextChanged: {
                    actions.close();
                    panel.glimpse.search(text);
                }
                Text {
                    anchors.fill: parent
                    verticalAlignment: Text.AlignVCenter
                    visible: input.text === "" && !input.preeditText
                    text: "Search apps, files, content and commands"
                    color: Style.dimText
                    font: input.font
                }
            }
        }

        Item {
            id: content
            visible: panel.expanded
            y: bar.height
            width: parent.width
            height: panel.expandedHeight - bar.height - status.height
            readonly property bool empty: panel.glimpse.current && panel.glimpse.results.complete && panel.glimpse.rows.length === 0 && (panel.glimpse.results.status || "") === ""

            ListView {
                id: list
                visible: !content.empty
                width: 300
                height: parent.height
                bottomMargin: 8
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                model: panel.glimpse.rows
                currentIndex: panel.glimpse.selected
                highlightFollowsCurrentItem: false
                onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)
                delegate: ResultRow {
                    width: list.width
                    rows: panel.glimpse.rows
                    selected: index === panel.glimpse.selected
                    onClicked: {
                        panel.glimpse.select(index);
                        panel.glimpse.activate(index, "enter");
                        panel.focusSearch();
                    }
                    onPressedRow: {
                        panel.glimpse.select(index);
                        panel.focusSearch();
                    }
                }
            }

            PreviewPane {
                id: preview
                visible: !content.empty && panel.glimpse.config.preview !== false
                x: list.width
                width: parent.width - list.width
                height: parent.height
                glimpse: panel.glimpse
                backend: panel.backend
                onTyped: (t, cursor) => {
                    input.text = t;
                    input.cursorPosition = cursor;
                    panel.focusSearch();
                }
                cursor: input.cursorPosition
                selectionStart: input.selectionStart
                selectionEnd: input.selectionEnd
                searchText: input.text
                onRefocus: panel.focusSearch()
            }

            Column {
                visible: content.empty
                anchors.centerIn: parent
                width: parent.width - 80
                spacing: 8
                Icon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    name: "system-search-symbolic"
                    size: 48
                    color: Style.tertiary
                }
                Label {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    text: "No results for “" + panel.text.trim() + "”"
                    role: "title"
                }
                Label {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    text: "Try fewer or different words, or add a folder to search."
                    role: "secondary"
                    dim: true
                    wrapMode: Text.Wrap
                }
            }
        }

        // Key hints for the selected result (or a message), the count, then the buttons.
        Item {
            id: status
            visible: panel.expanded
            y: panel.expandedHeight - height
            width: parent.width
            height: 36

            readonly property string note: panel.glimpse.message || (panel.glimpse.current ? (panel.glimpse.results.status || "") : "")

            Row {
                id: hints
                visible: status.note === ""
                anchors.left: parent.left
                anchors.leftMargin: 16
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12
                Repeater {
                    model: panel.glimpse.row ? panel.glimpse.row.hints : []
                    delegate: Row {
                        required property var modelData
                        spacing: 6
                        Row {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            Repeater {
                                model: modelData.keys
                                delegate: KeyCap {
                                    required property string modelData
                                    text: modelData
                                }
                            }
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: modelData.label
                            color: Style.dimText
                            font.family: Style.font
                            font.pointSize: 11.5 * 0.75
                        }
                    }
                }
            }
            Text {
                visible: status.note !== ""
                anchors.left: parent.left
                anchors.leftMargin: 16
                anchors.right: tools.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                text: status.note
                color: Style.text
                font.family: Style.font
                font.pointSize: 11.5 * 0.75
                elide: Text.ElideRight
            }

            Row {
                id: tools
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    rightPadding: 6
                    readonly property int n: panel.glimpse.rows.length
                    text: n === 0 ? "" : n === 1 ? "1 result" : n + " results"
                    color: Style.tertiary
                    font.family: Style.font
                    font.pointSize: 11.5 * 0.75
                    font.features: { "tnum": 1 }
                }
                StatusButton {
                    icon: "folder-new-symbolic"
                    tooltip: "Add a folder to search"
                    onClicked: panel.backend.request("AddFolder", {}, null)
                }
                StatusButton {
                    icon: "application-x-addon-symbolic"
                    tooltip: "Plugins"
                    onClicked: panel.backend.request("OpenPlugins", {}, null)
                }
                StatusButton {
                    icon: "view-more-symbolic"
                    tooltip: "Actions (" + actions.caption(panel.glimpse.config.action_key) + ")"
                    onClicked: if (panel.glimpse.row)
                        actions.openFor(panel.glimpse.row)
                }
                StatusButton {
                    icon: "nyx-settings-symbolic"
                    tooltip: "Settings"
                    onClicked: panel.backend.request("OpenSettings", {}, null)
                }
            }
        }
    }

    // The action panel (Ctrl+K) and the search history (Up on an empty search), under
    // the search bar.
    ActionPanel {
        id: actions
        glimpse: panel.glimpse
        backend: panel.backend
        x: Math.round((panel.width - width) / 2)
        y: bar.height - 4
        onClosed: panel.focusSearch()
        onPicked: q => {
            input.text = q;
            input.cursorPosition = q.length;
        }
    }
}
