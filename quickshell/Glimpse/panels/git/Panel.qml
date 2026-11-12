pragma ComponentBehavior: Bound
import QtQuick
import Nyx.Ui

// Nyx Git at a glance: the open (or last) repository's branch, how far it is from its
// upstream and what's changed, beside its branches, worktrees, recent commits or open
// requests. The words after `git` filter the list. Left and Right switch lists, Up and
// Down pick, Enter switches to the branch, opens the worktree or opens Git at the commit
// or request; Ctrl+O opens Git; Ctrl+F fetches. Every change goes through Git's own
// actions (switching asks Git, which refuses when it would overwrite local changes).
FocusScope {
    id: panel

    property var host: null
    readonly property bool wide: host !== null && host.compact
    readonly property var live: host ? host.topic("status") : ({})
    readonly property var s: live.state || ({})
    readonly property string filter: host ? host.query.trim().toLowerCase() : ""
    property string tab: "branches"
    property var requests: null
    property string requestsError: ""
    property bool busy: false
    readonly property var source: tab === "branches" ? (s.branches || [])
        : tab === "worktrees" ? (s.worktrees || [])
        : tab === "recent" ? (s.recent || [])
        : (requests || [])
    readonly property var shown: source.filter(i => filter === "" || (i.title || "").toLowerCase().indexOf(filter) >= 0)
    property int picked: 0
    readonly property var pickedOne: shown[Math.min(picked, shown.length - 1)] || null
    readonly property var tabs: [
        { id: "branches", label: "Branches" },
        { id: "worktrees", label: "Worktrees" },
        { id: "recent", label: "Recent" },
        { id: "requests", label: "Requests" }
    ]

    function choose(id) {
        tab = id;
        picked = 0;
        if (id === "requests" && requests === null)
            loadRequests();
    }
    function loadRequests() {
        requestsError = "";
        host.run("repo.reviews", { path: s.repository }, (result, err) => {
            if (err) {
                panel.requestsError = err;
                panel.requests = [];
                return;
            }
            panel.requests = (result && result.items) || [];
        });
    }
    function activate(item) {
        if (!item || !s.repository)
            return;
        if (tab === "branches") {
            if (item.current)
                return;
            busy = true;
            host.run("repo.switch_branch", { branch: item.value, path: s.repository }, (result, err) => {
                panel.busy = false;
                host.say(err ? err : result.message);
            });
        } else if (tab === "worktrees") {
            host.open({ path: item.path });
        } else if (tab === "recent") {
            host.open({ path: s.repository, page: "history" });
        } else {
            host.open({ path: s.repository, page: "review" });
        }
    }
    function fetch() {
        if (!s.repository)
            return;
        busy = true;
        host.run("repo.fetch", { path: s.repository }, (result, err) => {
            panel.busy = false;
            host.say(err ? err : result.message);
        });
    }

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        const i = tabs.findIndex(t => t.id === tab);
        if (e.key === Qt.Key_Down)
            picked = Math.min(shown.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (e.key === Qt.Key_Right)
            choose(tabs[Math.min(tabs.length - 1, i + 1)].id);
        else if (e.key === Qt.Key_Left)
            choose(tabs[Math.max(0, i - 1)].id);
        else if (ctrl && e.key === Qt.Key_O)
            host.open(s.repository ? { path: s.repository } : {});
        else if (ctrl && e.key === Qt.Key_F)
            fetch();
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            activate(pickedOne);
        else
            return;
        e.accepted = true;
    }

    // The repository: what Git would show in its header.
    Column {
        id: summary
        width: panel.wide ? 220 : parent.width
        spacing: 3
        visible: !!panel.s.repository
        Label {
            width: parent.width
            text: panel.s.name || ""
            role: "title"
            elide: Text.ElideRight
        }
        Label {
            width: parent.width
            text: (panel.s.branch || "") + (panel.s.upstream ? "  ↑" + panel.s.ahead + " ↓" + panel.s.behind + " " + panel.s.upstream : "  · not published")
            elide: Text.ElideRight
        }
        Label {
            width: parent.width
            text: panel.s.changed === 0 ? "Working tree clean"
                : panel.s.changed + " changed · " + panel.s.staged + " staged" + (panel.s.untracked ? " · " + panel.s.untracked + " untracked" : "")
            role: "caption"
            dim: true
            elide: Text.ElideRight
        }
        Row {
            spacing: 4
            Badge {
                visible: panel.s.conflicts > 0
                text: panel.s.conflicts + " conflicted"
                tone: "danger"
            }
            Badge {
                visible: !!panel.s.operation
                text: (panel.s.operation || "") + " in progress"
                tone: "warning"
            }
        }
        Item { width: 1; height: 6 }
        Row {
            spacing: 6
            Button {
                text: "Open Git"
                onClicked: panel.host.open({ path: panel.s.repository })
            }
            Button {
                text: "Changes"
                visible: panel.s.changed > 0
                onClicked: panel.host.open({ path: panel.s.repository, page: "changes" })
            }
            IconButton {
                icon: "network-receive-symbolic"
                tooltip: "Fetch all remotes (Ctrl+F)"
                onClicked: panel.fetch()
            }
            Spinner {
                visible: panel.busy
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }

    Item {
        id: lists
        x: panel.wide ? summary.width + 16 : 0
        y: panel.wide ? 0 : summary.height + 10
        width: parent.width - x
        height: parent.height - y
        visible: !!panel.s.repository
        Segmented {
            id: switcher
            width: parent.width
            options: panel.tabs
            current: panel.tab
            onChosen: id => panel.choose(id)
        }
        ListView {
            id: list
            anchors.top: switcher.bottom
            anchors.topMargin: 6
            anchors.bottom: parent.bottom
            width: parent.width
            clip: true
            model: panel.shown
            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                width: list.width
                height: 40
                radius: Style.radiusSmall
                color: index === panel.picked ? Style.selected : "transparent"
                Accessible.role: Accessible.ListItem
                Accessible.name: (modelData.title || "") + ", " + (modelData.subtitle || "")
                Accessible.selected: index === panel.picked
                Rectangle {
                    x: 10
                    anchors.verticalCenter: parent.verticalCenter
                    width: 7
                    height: 7
                    radius: 4
                    visible: panel.tab === "branches" || panel.tab === "worktrees"
                    color: row.modelData.current || row.modelData.main ? Style.accent : (row.modelData.changed > 0 ? Style.warning : Style.dimText)
                }
                Column {
                    x: 26
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 34
                    Label {
                        width: parent.width
                        text: row.modelData.title || ""
                        elide: Text.ElideRight
                    }
                    Label {
                        width: parent.width
                        text: row.modelData.subtitle || ""
                        role: "caption"
                        dim: true
                        elide: Text.ElideMiddle
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    onClicked: panel.picked = row.index
                    onDoubleClicked: panel.activate(row.modelData)
                }
            }
            StateView {
                anchors.centerIn: parent
                width: parent.width
                visible: panel.shown.length === 0
                kind: panel.tab === "requests" && panel.requests === null ? "loading" : (panel.requestsError !== "" && panel.tab === "requests" ? "error" : "empty")
                icon: "media-playlist-consecutive-symbolic"
                text: panel.tab === "requests" && panel.requestsError !== "" ? panel.requestsError
                    : panel.filter !== "" ? "Nothing matching “" + panel.host.query + "”"
                    : panel.tab === "requests" ? "No open requests" : "Nothing here"
            }
        }
    }

    StateView {
        anchors.centerIn: parent
        width: parent.width
        visible: !panel.s.repository && panel.live.state !== undefined
        kind: panel.live.error ? "error" : "empty"
        icon: "folder-symbolic"
        text: panel.live.error ? panel.live.error : "No repository open in Git yet"
    }
}
