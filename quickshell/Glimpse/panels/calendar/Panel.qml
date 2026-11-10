import QtQuick
import Nyx.Ui

// Calendar at a glance: what's coming up, by day. The words after `calendar` are an
// event to add (`calendar lunch with Sam tomorrow at 1`): Enter or Add puts it in the
// usual calendar. Up and Down pick an event, Ctrl+O opens Calendar at its day.
FocusScope {
    id: panel

    property var host: null
    readonly property var live: host ? host.topic("upcoming") : ({})
    readonly property var upcoming: live.state || ({})
    readonly property string failure: upcoming.error || ""
    readonly property string words: host ? host.query.trim() : ""
    property int picked: 0
    property bool adding: false

    // "2026-11-09T14:30" or "2026-11-09", in local time.
    function parse(s) {
        const p = (s || "").split(/[-T:]/).map(Number);
        return new Date(p[0], (p[1] || 1) - 1, p[2] || 1, p[3] || 0, p[4] || 0);
    }
    function dayName(d) {
        const today = new Date();
        today.setHours(0, 0, 0, 0);
        const days = Math.round((new Date(d.getFullYear(), d.getMonth(), d.getDate()) - today) / 86400000);
        if (days === 0)
            return "Today";
        if (days === 1)
            return "Tomorrow";
        return d.toLocaleDateString(Qt.locale(), days < 7 ? "dddd" : "ddd d MMMM");
    }
    function time(e) {
        if (e.all_day)
            return "All day";
        return parse(e.start).toLocaleTimeString(Qt.locale(), Locale.ShortFormat) + " – " + parse(e.end).toLocaleTimeString(Qt.locale(), Locale.ShortFormat);
    }
    // Headings for each day, then its events.
    readonly property var rows: {
        const out = [];
        let last = "";
        (upcoming.events || []).filter(e => !e.cancelled).forEach(e => {
            const day = dayName(parse(e.start));
            if (day !== last)
                out.push({ heading: day });
            last = day;
            out.push({ event: e });
        });
        return out;
    }
    readonly property var eventRows: rows.map((r, i) => r.event ? i : -1).filter(i => i >= 0)
    readonly property var pickedEvent: eventRows.length > 0 ? rows[eventRows[Math.min(picked, eventRows.length - 1)]].event : null

    function add() {
        if (words === "" || adding)
            return;
        adding = true;
        host.run("calendar.add", { text: words }, (result, err) => {
            panel.adding = false;
            if (result)
                host.say("Added " + result.title + ", " + dayName(parse(result.start)).toLowerCase() + (result.all_day ? "" : " at " + parse(result.start).toLocaleTimeString(Qt.locale(), Locale.ShortFormat)));
        });
    }
    function openAt(e) {
        host.open(e ? { date: e.start.slice(0, 10) } : {});
    }

    Keys.onPressed: e => {
        const ctrl = (e.modifiers & Qt.ControlModifier) !== 0;
        if (e.key === Qt.Key_Down)
            picked = Math.min(eventRows.length - 1, picked + 1);
        else if (e.key === Qt.Key_Up)
            picked = Math.max(0, picked - 1);
        else if ((e.key === Qt.Key_Return || e.key === Qt.Key_Enter) && words !== "")
            add();
        else if ((e.key === Qt.Key_Return || e.key === Qt.Key_Enter) || (ctrl && e.key === Qt.Key_O))
            openAt(pickedEvent);
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
        actionText: "Open Calendar"
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
            name: "appointment-new-symbolic"
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

    ListView {
        id: list
        visible: panel.failure === ""
        anchors.top: addBar.bottom
        anchors.topMargin: addBar.visible ? 8 : 0
        anchors.bottom: footer.top
        anchors.bottomMargin: 6
        width: parent.width
        clip: true
        model: panel.rows
        delegate: Item {
            id: row
            required property var modelData
            required property int index
            readonly property var e: modelData.event || null
            readonly property bool chosen: e !== null && panel.eventRows[panel.picked] === index
            width: list.width
            height: e ? 46 : 28
            Label {
                visible: !row.e
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 4
                text: row.modelData.heading || ""
                role: "caption"
                dim: true
            }
            Rectangle {
                visible: row.e !== null
                anchors.fill: parent
                radius: Style.radiusSmall
                color: row.chosen ? Style.selected : "transparent"
                Rectangle {
                    x: 10
                    y: 9
                    width: 3
                    height: parent.height - 18
                    radius: 1.5
                    color: Style.accent
                }
                Column {
                    x: 22
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 32
                    Label {
                        width: parent.width
                        text: row.e ? row.e.title || "Untitled" : ""
                        elide: Text.ElideRight
                    }
                    Label {
                        width: parent.width
                        text: row.e ? panel.time(row.e) + (row.e.location ? " · " + row.e.location : "") + (row.e.calendar_name ? " · " + row.e.calendar_name : "") : ""
                        role: "caption"
                        dim: true
                        elide: Text.ElideRight
                    }
                }
                MouseArea {
                    anchors.fill: parent
                    onClicked: panel.picked = panel.eventRows.indexOf(row.index)
                    onDoubleClicked: panel.openAt(row.e)
                }
            }
        }
        StateView {
            anchors.centerIn: parent
            width: parent.width
            visible: panel.rows.length === 0 && panel.upcoming.events !== undefined
            kind: "empty"
            icon: "x-office-calendar-symbolic"
            text: "Nothing coming up"
        }
    }

    Item {
        id: footer
        anchors.bottom: parent.bottom
        width: parent.width
        height: openCalendar.height
        Button {
            id: openCalendar
            anchors.right: parent.right
            visible: panel.failure === ""
            text: "Open Calendar"
            onClicked: panel.openAt(panel.pickedEvent)
        }
    }
}
