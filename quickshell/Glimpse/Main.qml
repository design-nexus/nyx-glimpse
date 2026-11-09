import QtQuick
import Quickshell
import Nyx.Shell
import Nyx.Ui

// Glimpse in the shared Quickshell runtime (nyx-shell-qs). Search, ranking and
// launching stay in nyx-glimpse-backend; this draws its panel: a search field and the
// results, centred near the top of the active screen over a clear catcher that closes
// it on a click anywhere else.
//
// Shell Core decides when the surface shows (a transient overlay with the keyboard,
// hidden while the session is locked, focus given back on close); the backend says
// when the person asked (`nyx-glimpse --toggle`, Super+Space). Replies for an older
// query than the one in the field are dropped.
Scope {
    id: root

    BackendClient {
        id: backend
        app: "nyx-glimpse"
        topics: ["panel", "results"]
    }

    readonly property var panelState: backend.state.panel || ({ open: false, serial: 0 })
    readonly property var results: backend.state.results || ({ generation: 0, rows: [], status: "", complete: true })
    readonly property bool asked: panelState.open === true && backend.connected
    readonly property bool current: results.generation === generation
    readonly property var rows: current ? results.rows : []

    property int generation: 0
    property int selected: 0
    // The result waiting for a second Enter, and what the panel says about it.
    property string armed: ""
    property string message: ""

    function hide() {
        backend.request("Hide", {}, null);
    }
    function search(text) {
        generation += 1;
        selected = 0;
        armed = "";
        message = "";
        backend.request("Query", { text: text, generation: generation }, null);
    }
    function activate(index) {
        const row = rows[index];
        if (!row)
            return;
        const confirmed = armed === row.id;
        message = "";
        backend.request("Activate", { id: row.id, generation: generation, confirmed: confirmed }, (ok, err) => {
            if (err) {
                armed = "";
                message = err.msg;
            } else if (ok.ask) {
                armed = row.id;
                message = ok.ask;
            } else if (ok.message) {
                armed = "";
                message = ok.message;
            } else {
                armed = "";
                root.hide();
            }
        });
    }

    // Tests: GLIMPSE_TEST_QUERY is typed into the field when the panel opens.
    readonly property string testQuery: Quickshell.env("GLIMPSE_TEST_QUERY") || ""

    onAskedChanged: {
        if (asked) {
            field.clear();
            if (testQuery !== "")
                field.text = testQuery;
            else
                search("");
            ShellCore.showOverlay("glimpse");
        } else {
            ShellCore.hideOverlay("glimpse");
        }
    }
    onRowsChanged: if (selected >= rows.length)
        selected = Math.max(0, rows.length - 1)

    OverlaySurface {
        id: surface
        overlayId: "glimpse"
        category: "transient"
        group: "launcher"
        keyboard: "exclusive"
        // Asked for on purpose, it shows over a fullscreen window, as the GTK panel does.
        fullscreenRule: "show"
        placement: "active"
        restoreFocus: true
        anchors.top: true
        anchors.bottom: true
        anchors.left: true
        anchors.right: true

        // Closed by Shell Core (Escape, the lock, another launcher): the backend hears it,
        // so the next toggle opens again.
        onShownChanged: {
            if (shown)
                field.focusInput();
            else if (root.asked)
                root.hide();
        }

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
            onPressed: root.hide()
        }

        Surface {
            id: card
            width: Math.min(760, parent.width - 48)
            height: column.implicitHeight + 2 * Style.padding
            anchors.horizontalCenter: parent.horizontalCenter
            y: Math.round(parent.height * 0.18)
            opacity: surface.shown ? 1 : 0
            Behavior on opacity {
                NumberAnimation {
                    duration: Style.reducedMotion ? 0 : Style.fast
                }
            }

            MouseArea {
                anchors.fill: parent
            }

            Column {
                id: column
                x: Style.padding
                y: Style.padding
                width: parent.width - 2 * Style.padding
                spacing: Style.spacing

                SearchField {
                    id: field
                    width: parent.width
                    placeholder: "Search apps, files and actions"
                    onTextChanged: root.search(text)
                    onAccepted: root.activate(root.selected)
                    onKeyDown: root.selected = Math.min(root.selected + 1, root.rows.length - 1)
                    onKeyUp: root.selected = Math.max(root.selected - 1, 0)
                }

                ListView {
                    id: list
                    width: parent.width
                    height: Math.min(contentHeight, 9 * (Style.rowHeight + 10))
                    visible: root.rows.length > 0
                    clip: true
                    spacing: 2
                    model: root.rows
                    currentIndex: root.selected
                    highlightFollowsCurrentItem: false
                    boundsBehavior: Flickable.StopAtBounds
                    onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)
                    delegate: ListRow {
                        required property var modelData
                        required property int index
                        width: list.width
                        icon: modelData.icon
                        iconSize: modelData.kind === "app" ? 24 : 18
                        title: modelData.title
                        subtitle: modelData.subtitle
                        selected: index === root.selected
                        activeFocusOnTab: false
                        onClicked: {
                            root.selected = index;
                            root.activate(index);
                        }
                        Badge {
                            text: modelData.reason
                            tone: "neutral"
                        }
                    }
                }

                Label {
                    width: parent.width
                    visible: text !== ""
                    text: root.message || (root.current ? root.results.status : "")
                    role: "secondary"
                    dim: true
                    elide: Text.ElideRight
                }

                Label {
                    width: parent.width
                    visible: field.text.trim() !== "" && root.current && root.results.complete && root.rows.length === 0
                    text: "No results"
                    role: "secondary"
                    dim: true
                }
            }
        }
    }
}
