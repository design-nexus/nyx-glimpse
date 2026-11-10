import QtQuick
import Quickshell
import Quickshell.Io
import Nyx.Shell
import Nyx.Ui

// Glimpse in the shared Quickshell runtime (nyx-shell-qs), as the GTK panel draws it:
// a search bar that grows into results on the left (under category headings) and the
// selected one previewed on the right, with key hints, the count and the panel's
// buttons along the bottom. Search, ranking, previews and every action stay in
// nyx-glimpse-backend; this only draws and sends keys.
//
// Shell Core (or the runtime's standalone mode on Omarchy) decides when the surface
// shows: a transient overlay with the keyboard, hidden while the session is locked,
// focus given back on close. The backend says when the person asked (`nyx-glimpse
// --toggle`, Super+Space). Replies for an older query than the one in the field are
// dropped.
Scope {
    id: root

    BackendClient {
        id: backend
        app: "nyx-glimpse"
        topics: ["panel", "results", "config", "plugins", "manager"]
        // A backend that started again knows no search: the one on show is sent again.
        onReconnected: if (panel.text.trim() !== "")
            root.search(panel.text)
        onEvent: (name, body) => {
            if (name === "message")
                root.say(body.text || "");
            else if (name === "rerun")
                root.search(panel.text);
        }
    }

    readonly property var panelState: backend.state.panel || ({ open: false, serial: 0 })
    readonly property var results: backend.state.results || ({ generation: 0, rows: [], status: "", complete: true, calc: null })
    readonly property var config: backend.state.config || ({ preview: true, action_key: "<Control>k", favorite_key: "<Control><Shift>f" })
    readonly property bool asked: panelState.open === true && backend.connected
    readonly property bool current: results.generation === generation
    readonly property var rows: current ? results.rows : []
    readonly property var row: rows[selected] || null
    readonly property bool expanded: panel.text.trim() !== ""

    property int generation: 0
    property int selected: 0
    // Moved through the list by key or click: late results keep the selection.
    property bool browsing: false
    property string selectedId: ""
    // The result waiting for a second Enter, and what the status line says.
    property string armed: ""
    property string message: ""
    // A command's result is in its preview: the pane loads it again.
    property int previewSerial: 0

    function hide() {
        backend.request("Hide", {}, null);
    }
    function search(text) {
        generation += 1;
        selected = 0;
        browsing = false;
        armed = "";
        message = "";
        backend.request("Query", { text: text, generation: generation }, null);
    }
    function say(text) {
        message = text;
        if (text === "Copied")
            unsay.restart();
    }
    Timer {
        id: unsay
        interval: 2000
        onTriggered: if (root.message === "Copied")
            root.message = ""
    }
    function select(index) {
        if (rows.length === 0)
            return;
        selected = Math.max(0, Math.min(rows.length - 1, index));
        browsing = true;
    }
    // Enter (`how`: enter, reveal, alt, copy, site, or an action-panel verb).
    function activate(index, how) {
        const r = rows[index];
        if (!r)
            return;
        const confirmed = armed === r.id;
        if (how !== "copy")
            message = "";
        backend.request("Activate", { id: r.id, generation: generation, how: how, confirmed: confirmed }, (ok, err) => {
            if (err) {
                armed = "";
                say(err.msg);
            } else if (ok.ask) {
                armed = r.id;
                say(ok.ask);
                if (ok.preview)
                    previewSerial += 1;
            } else if (ok.done) {
                armed = "";
                root.hide();
            } else if (ok.focus) {
                // A panel: the keyboard goes into it; Escape gives it back.
                panel.focusMiniApp();
            } else if (ok.fill !== undefined) {
                // A command that needs more words: the search becomes it, ready to type on.
                armed = "";
                panel.setText(ok.fill);
            } else {
                armed = "";
                if (ok.message)
                    say(ok.message);
                if (ok.preview)
                    previewSerial += 1;
                if (ok.rerun)
                    root.search(panel.text);
            }
        });
    }

    // Moving away from a result that asked disarms it.
    onSelectedChanged: {
        const r = rows[selected];
        selectedId = r ? r.id : "";
        if (armed !== "" && selectedId !== armed) {
            armed = "";
            message = "";
        }
    }
    // New results: the top one, unless the person is moving through the list.
    onRowsChanged: {
        const keep = browsing ? rows.findIndex(r => r.id === selectedId) : -1;
        selected = keep >= 0 ? keep : 0;
        const r = rows[selected];
        selectedId = r ? r.id : "";
        if (!browsing)
            panel.scrollTop();
    }

    // Tests: GLIMPSE_TEST_QUERY is typed into the field when the panel opens.
    readonly property string testQuery: Quickshell.env("GLIMPSE_TEST_QUERY") || ""

    property int seenSerial: -1
    onPanelStateChanged: {
        if (panelState.open !== true || panelState.serial === seenSerial)
            return;
        seenSerial = panelState.serial;
        if (panelState.query)
            panel.setText(panelState.query);
        else if (!panelState.keep) {
            if (testQuery !== "")
                panel.setText(testQuery);
            else
                panel.setText("");
        }
    }
    onAskedChanged: {
        if (asked) {
            ShellCore.showOverlay("glimpse");
        } else {
            panel.closeActions();
            ShellCore.hideOverlay("glimpse");
        }
    }

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
                panel.focusSearch();
            else if (root.asked)
                root.hide();
        }

        // A click beside the panel puts it away; moving the pointer off it doesn't.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
            onPressed: root.hide()
        }

        Panel {
            id: panel
            glimpse: root
            backend: backend
            // Centred across; the search bar stays where the expanded panel would sit
            // centred, so results grow downwards from it.
            maxWidth: parent.width - 48
            maxHeight: parent.height - 48
            x: Math.round((parent.width - width) / 2)
            y: Math.max(0, Math.min(Math.round((parent.height - expandedHeight) / 2), parent.height - height))
            opacity: surface.shown ? 1 : 0
            Behavior on opacity {
                NumberAnimation {
                    duration: Style.reducedMotion ? 0 : Style.fast
                }
            }
        }
    }

    PluginWindow {
        backend: backend
    }

    // Tests (GLIMPSE_TEST_IPC=1): `qs ipc --pid N call glimpse <fn>` stands in for the
    // keyboard on a virtual output.
    IpcHandler {
        target: "glimpse"
        enabled: Quickshell.env("GLIMPSE_TEST_IPC") === "1"
        function select(i: int): void {
            root.select(i);
        }
        function actions(): void {
            if (root.row)
                panel.openActions(root.row);
        }
        function history(): void {
            panel.openHistory();
        }
        function type(text: string): void {
            panel.setText(text);
        }
        function enter(how: string): void {
            root.activate(root.selected, how || "enter");
        }
        // One of the shown mini-app panel's own functions, as its key would (`add`).
        function mini(fn: string): void {
            panel.miniApp(fn);
        }
    }
}
