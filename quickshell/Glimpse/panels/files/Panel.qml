import QtQuick
import Nyx.Ui

// Files at a glance: the words after `files` open a place or folder (`files downloads`)
// or search the home folder by name. Up and Down pick, Enter opens a file or goes into
// a folder, Backspace or Left goes up; Ctrl+C or Ctrl+X marks for copying or moving,
// Ctrl+V puts them here, Delete moves to the Trash (asking first), Ctrl+O opens the
// folder in Files. Every change goes through Files' own operations.
FocusScope {
    id: panel

    property var host: null
    property string path: ""
    property string shown: ""
    property string parentPath: ""
    property var items: []
    property var places: []
    property bool searching: false
    property bool loading: true
    property string failure: ""
    property int picked: 0
    // Marked with Ctrl+C or Ctrl+X: {move, paths}.
    property var marked: null
    readonly property var pickedItem: items[picked] || null
    // Files says when a folder it listed changed; the one shown lists again.
    readonly property var changes: (host && host.topic("changes").state && host.topic("changes").state.changed) || ({})
    property int seen: 0
    onChangesChanged: {
        const n = changes[path];
        if (path && !searching && n !== undefined && n > seen) {
            seen = n;
            reload();
        }
    }
    function reload() {
        host.run("files.list", { path: path }, (result, err) => {
            if (!result || result.path !== panel.path)
                return;
            items = result.items || [];
            picked = Math.min(picked, Math.max(0, items.length - 1));
        });
    }

    function go(target) {
        loading = true;
        host.run("files.list", { path: target }, (result, err) => {
            loading = false;
            if (err) {
                // Not a place or a folder: look for it by name.
                if (!panel.path)
                    find(target);
                else
                    failure = err;
                return;
            }
            failure = "";
            searching = false;
            seen = panel.changes[result.path] || 0;
            path = result.path;
            shown = result.shown;
            parentPath = result.parent || "";
            items = result.items || [];
            picked = 0;
        });
    }
    function find(words) {
        loading = true;
        searching = true;
        host.run("files.search", { query: words }, (result, err) => {
            loading = false;
            failure = err || "";
            path = "";
            shown = "Matching “" + words + "”";
            parentPath = "";
            items = result ? (result.items || []) : [];
            picked = 0;
        });
    }
    function activate(item) {
        if (!item)
            return;
        if (item.kind === "folder")
            go(item.path);
        else
            host.run("files.open", { path: item.path }, (result, err) => {
                if (!err)
                    host.say("Opened " + item.title);
            });
    }
    function paste() {
        if (!marked || !path)
            return;
        const m = marked;
        host.run(m.move ? "files.move" : "files.copy", { sources: m.paths, destination: path }, (result, err) => {
            if (result) {
                host.say(result.message);
                panel.marked = null;
                go(path);
            }
        });
    }
    function trash(item) {
        if (!item)
            return;
        host.run("files.trash", { paths: [item.path] }, (result, err) => {
            if (result) {
                host.say(result.message);
                go(path);
            }
        });
    }
    // The breadcrumb's folders: each part of the shown path with the folder it names.
    readonly property var crumbs: {
        if (!path || searching)
            return [];
        const parts = shown.split("/").filter(p => p !== "");
        const full = path.split("/").filter(p => p !== "");
        const out = [];
        for (let i = 0; i < parts.length; i++)
            out.push({ name: parts[i], path: "/" + full.slice(0, full.length - parts.length + i + 1).join("/") });
        return out;
    }

    Component.onCompleted: {
        host.run("files.locations", {}, (result, err) => panel.places = result ? result.items : []);
        go(host.query !== "" ? host.query : "~");
    }

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        if (e.key === Qt.Key_Down)
            picked = Math.min(items.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            activate(pickedItem);
        else if ((e.key === Qt.Key_Backspace || e.key === Qt.Key_Left) && parentPath)
            go(parentPath);
        else if (ctrl && (e.key === Qt.Key_C || e.key === Qt.Key_X) && pickedItem) {
            marked = { move: e.key === Qt.Key_X, paths: [pickedItem.path] };
            host.say((marked.move ? "Move " : "Copy ") + pickedItem.title + ": Ctrl+V in a folder puts it there");
        } else if (ctrl && e.key === Qt.Key_V)
            paste();
        else if (e.key === Qt.Key_Delete)
            trash(pickedItem);
        else if (ctrl && e.key === Qt.Key_O)
            host.open({ path: path || "~" });
        else
            return;
        e.accepted = true;
    }

    Flow {
        id: placesRow
        width: parent.width
        spacing: 6
        Repeater {
            model: panel.places
            delegate: Button {
                required property var modelData
                text: modelData.title
                flat: modelData.path !== panel.path
                onClicked: panel.go(modelData.path)
            }
        }
    }

    Row {
        id: crumbRow
        anchors.top: placesRow.bottom
        anchors.topMargin: 10
        width: parent.width
        spacing: 2
        Label {
            visible: panel.searching || panel.crumbs.length === 0
            text: panel.shown
            role: "caption"
            dim: true
        }
        Repeater {
            model: panel.crumbs
            delegate: Row {
                required property var modelData
                required property int index
                spacing: 2
                Label {
                    visible: index > 0
                    text: "›"
                    role: "caption"
                    dim: true
                }
                Label {
                    text: modelData.name
                    role: "caption"
                    strong: index === panel.crumbs.length - 1
                    dim: index !== panel.crumbs.length - 1
                    MouseArea {
                        anchors.fill: parent
                        onClicked: panel.go(parent.parent.modelData.path)
                    }
                }
            }
        }
    }

    readonly property bool picture: pickedItem !== null && (pickedItem.type || "").startsWith("image/")

    ListView {
        id: list
        anchors.top: crumbRow.bottom
        anchors.topMargin: 6
        anchors.bottom: footer.top
        anchors.bottomMargin: 6
        width: panel.picture ? parent.width - 172 : parent.width
        clip: true
        model: panel.items
        currentIndex: panel.picked
        onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)
        delegate: ListRow {
            required property var modelData
            required property int index
            width: list.width
            icon: modelData.icon || (modelData.kind === "folder" ? "folder-symbolic" : "text-x-generic-symbolic")
            title: modelData.title
            subtitle: panel.searching ? modelData.path : modelData.subtitle
            selected: index === panel.picked
            onClicked: {
                if (panel.picked === index)
                    panel.activate(modelData);
                panel.picked = index;
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.loading || panel.failure !== "" || panel.items.length === 0
            kind: panel.loading ? "loading" : panel.failure !== "" ? "error" : "empty"
            icon: "folder-symbolic"
            text: panel.loading ? "" : panel.failure !== "" ? panel.failure : panel.searching ? "Nothing in your home folder is called that" : "This folder is empty"
        }
    }

    Rectangle {
        visible: panel.picture
        anchors.top: list.top
        anchors.right: parent.right
        width: 160
        height: 160
        radius: Style.radiusCard
        color: Style.card
        clip: true
        Image {
            anchors.fill: parent
            anchors.margins: 6
            source: panel.picture ? "file://" + panel.pickedItem.path : ""
            fillMode: Image.PreserveAspectFit
            asynchronous: true
            sourceSize.width: 320
            sourceSize.height: 320
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openFiles.height
        Row {
            spacing: 6
            Button {
                visible: panel.marked !== null && panel.path !== ""
                text: panel.marked ? (panel.marked.move ? "Move here" : "Copy here") : ""
                primary: true
                onClicked: panel.paste()
            }
            Button {
                visible: panel.parentPath !== ""
                text: "Up"
                onClicked: panel.go(panel.parentPath)
            }
        }
        Button {
            id: openFiles
            anchors.right: parent.right
            text: "Open in Files"
            onClicked: panel.host.open({ path: panel.path || "~" })
        }
    }
}
