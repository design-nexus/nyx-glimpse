import QtQuick
import Nyx.Ui

// Reminders due today or overdue. The words after `reminders` are a reminder to add,
// and may say when (`reminders call Sam tomorrow at 9`): Enter or Add puts it in the
// usual list. Up and Down pick one, Space marks it done, Ctrl+O opens Reminders.
FocusScope {
    id: panel

    property var host: null
    readonly property var live: host ? host.topic("today") : ({})
    readonly property var today: live.state || ({})
    readonly property string failure: today.error || ""
    readonly property string words: host ? host.query.trim() : ""
    readonly property var tasks: (today.reminders || []).filter(t => !t.done)
    property int picked: 0
    property bool adding: false
    readonly property var pickedTask: tasks.length > 0 ? tasks[Math.min(picked, tasks.length - 1)] : null

    // "2026-11-09T09:00" or "2026-11-09".
    function due(t) {
        if (!t.due)
            return "";
        const p = t.due.split(/[-T:]/).map(Number);
        const d = new Date(p[0], p[1] - 1, p[2], p[3] || 0, p[4] || 0);
        const now = new Date();
        const sameDay = d.toDateString() === now.toDateString();
        const day = sameDay ? "Today" : d.toLocaleDateString(Qt.locale(), "ddd d MMM");
        return t.due.length > 10 ? day + " " + d.toLocaleTimeString(Qt.locale(), Locale.ShortFormat) : day;
    }
    function add() {
        if (words === "" || adding)
            return;
        adding = true;
        host.run("reminders.create", { title: words }, (result, err) => {
            panel.adding = false;
            if (result)
                host.say("Added “" + panel.words + "”");
        });
    }
    function complete(t) {
        t = t || pickedTask;
        if (!t)
            return;
        host.run("reminders.complete", { list: t.list, uid: t.uid }, (result, err) => {
            if (result)
                host.say("Done: " + t.title);
        });
    }

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        if (e.key === Qt.Key_Down)
            picked = Math.min(tasks.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if ((e.key === Qt.Key_Return || e.key === Qt.Key_Enter) && words !== "")
            add();
        else if (e.key === Qt.Key_Space)
            complete(pickedTask);
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
        actionText: "Open Reminders"
        onAction: panel.host.open({})
    }

    Rectangle {
        id: addBar
        visible: panel.failure === "" && panel.words !== ""
        width: parent.width
        height: visible ? 44 : 0
        radius: Style.radiusSmall
        color: Style.card
        Icon {
            x: 12
            anchors.verticalCenter: parent.verticalCenter
            name: "list-add-symbolic"
            size: 18
            color: Style.accent
        }
        Label {
            x: 40
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - 40 - addButton.width - 20
            text: "Add “" + panel.words + "”"
            elide: Text.ElideRight
        }
        Button {
            id: addButton
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            text: panel.adding ? "Adding…" : "Add"
            primary: true
            onClicked: panel.add()
        }
    }

    Label {
        id: heading
        visible: panel.failure === "" && panel.tasks.length > 0
        anchors.top: addBar.bottom
        anchors.topMargin: addBar.visible ? 10 : 0
        text: "Today"
        role: "caption"
        dim: true
    }

    ListView {
        id: list
        visible: panel.failure === ""
        anchors.top: heading.bottom
        anchors.topMargin: 4
        anchors.bottom: footer.top
        anchors.bottomMargin: 6
        width: parent.width
        clip: true
        model: panel.tasks
        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index
            width: list.width
            height: 44
            radius: Style.radiusSmall
            color: index === panel.picked ? Style.selected : "transparent"
            Accessible.role: Accessible.ListItem
            Accessible.name: [modelData.title, panel.due(modelData), modelData.overdue ? "overdue" : "", modelData.list_name].filter(s => s).join(", ")
            Accessible.selected: index === panel.picked
            IconButton {
                id: check
                x: 4
                anchors.verticalCenter: parent.verticalCenter
                icon: "checkbox-symbolic"
                tooltip: "Mark " + row.modelData.title + " done (Space)"
                onClicked: panel.complete(row.modelData)
            }
            Column {
                anchors.left: check.right
                anchors.leftMargin: 6
                anchors.right: parent.right
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                Label {
                    width: parent.width
                    text: row.modelData.title
                    elide: Text.ElideRight
                }
                Label {
                    width: parent.width
                    text: [panel.due(row.modelData), row.modelData.list_name].filter(s => s).join(" · ")
                    role: "caption"
                    color: row.modelData.overdue ? Style.danger : Style.dimText
                    elide: Text.ElideRight
                }
            }
            MouseArea {
                anchors.fill: parent
                z: -1
                onClicked: panel.picked = row.index
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.tasks.length === 0 && panel.today.reminders !== undefined
            kind: "empty"
            icon: "object-select-symbolic"
            text: "Nothing due today"
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openReminders.height
        Button {
            id: openReminders
            anchors.right: parent.right
            visible: panel.failure === ""
            text: "Open Reminders"
            onClicked: panel.host.open({})
        }
    }
}
