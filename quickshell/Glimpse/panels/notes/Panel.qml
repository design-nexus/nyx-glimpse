import QtQuick
import Nyx.Ui

// Notes: the words after `notes` search the open vault as you type. Up and Down pick a
// note, Enter opens it in Notes, Ctrl+N proposes a new note with the words as its title
// (Notes shows the review and creates it only once accepted). Notes must be open, with
// its integration permissions on; otherwise this says so.
FocusScope {
    id: panel

    property var host: null
    // Across the top of Glimpse: what's searched and the buttons on the left, the notes
    // beside them.
    readonly property bool wide: host !== null && host.compact
    readonly property int listX: wide ? footer.width + 16 : 0
    readonly property string words: host ? host.query.trim() : ""
    property var hits: []
    property string failure: ""
    property bool searching: false
    property string searched: ""
    property int picked: 0
    readonly property var pickedHit: hits[picked] || null

    function search() {
        if (words === "") {
            hits = [];
            searched = "";
            failure = "";
            return;
        }
        const asked = words;
        searching = true;
        host.run("notes.vault_search", { query: asked }, (result, err) => {
            if (asked !== panel.words)
                return;
            panel.searching = false;
            panel.failure = err || "";
            panel.searched = asked;
            panel.hits = result ? (result.results || []).slice(0, 30) : [];
            panel.picked = 0;
        });
    }
    function title(path) {
        const name = (path || "").split("/").pop();
        return name.replace(/\.md$/i, "");
    }
    function open(hit) {
        hit = hit || pickedHit;
        if (hit)
            host.run("notes.vault_open", { note: hit.path }, (result, err) => {
                if (!err)
                    host.say("Opened " + title(hit.path));
            });
    }
    function propose() {
        if (words === "")
            return;
        host.run("notes.vault_create", { title: words, text: "" }, (result, err) => {
            if (result)
                host.say("Notes is showing the new note to review");
        });
    }

    Timer {
        id: typing
        interval: 250
        onTriggered: panel.search()
    }
    onWordsChanged: typing.restart()
    Component.onCompleted: search()

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        if (e.key === Qt.Key_Down)
            picked = Math.min(hits.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
            open(pickedHit);
        else if (ctrl && e.key === Qt.Key_N)
            propose();
        else if (ctrl && e.key === Qt.Key_O)
            host.open({});
        else
            return;
        e.accepted = true;
    }

    StateView {
        anchors.centerIn: parent
        width: parent.width
        visible: panel.failure !== ""
        kind: "error"
        text: panel.failure
        actionText: "Open Notes"
        onAction: panel.host.open({})
    }

    Column {
        visible: panel.wide && panel.failure === ""
        width: footer.width
        spacing: 2
        Label {
            text: "Notes"
            role: "title"
        }
        Label {
            width: parent.width
            text: panel.words === "" ? "Searches your notes as you type" : panel.searching ? "Searching…" : panel.hits.length + (panel.hits.length === 1 ? " note matches" : " notes match")
            dim: true
            wrapMode: Text.Wrap
        }
    }

    StateView {
        anchors.centerIn: parent
        anchors.horizontalCenterOffset: panel.listX / 2
        width: parent.width - panel.listX
        visible: panel.failure === "" && panel.words === ""
        kind: "empty"
        icon: "edit-find-symbolic"
        text: "Type after “notes” to search your notes"
    }

    ListView {
        id: list
        visible: panel.failure === "" && panel.words !== ""
        x: panel.listX
        anchors.top: parent.top
        anchors.bottom: panel.wide ? parent.bottom : footer.top
        anchors.bottomMargin: panel.wide ? 0 : 6
        width: parent.width - x
        clip: true
        model: panel.hits
        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index
            width: list.width
            height: 52
            radius: Style.radiusSmall
            color: index === panel.picked ? Style.selected : "transparent"
            Accessible.role: Accessible.ListItem
            Accessible.name: panel.title(modelData.path)
            Accessible.description: modelData.snippet || ""
            Accessible.selected: index === panel.picked
            Icon {
                x: 10
                y: 9
                name: "text-x-generic-symbolic"
                size: 18
            }
            Column {
                x: 38
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 48
                Label {
                    width: parent.width
                    text: panel.title(row.modelData.path)
                    elide: Text.ElideRight
                }
                Label {
                    width: parent.width
                    text: (row.modelData.snippet || row.modelData.path).replace(/\s+/g, " ").replace(/^#+ /, "")
                    role: "caption"
                    dim: true
                    elide: Text.ElideRight
                }
            }
            MouseArea {
                anchors.fill: parent
                onClicked: panel.picked = row.index
                onDoubleClicked: panel.open(row.modelData)
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.hits.length === 0 && panel.searched === panel.words && !panel.searching
            kind: "empty"
            icon: "edit-find-symbolic"
            text: "No notes match “" + panel.words + "”"
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: panel.wide ? 200 : parent.width
        height: panel.wide && newNote.visible ? newNote.height + 6 + openNotes.height : openNotes.height
        Button {
            id: newNote
            visible: panel.failure === "" && panel.words !== ""
            text: panel.wide ? "New note" : "New note “" + panel.words + "”"
            onClicked: panel.propose()
        }
        Button {
            id: openNotes
            anchors.bottom: parent.bottom
            anchors.right: panel.wide ? undefined : parent.right
            text: "Open Notes"
            onClicked: panel.host.open({})
        }
    }
}
