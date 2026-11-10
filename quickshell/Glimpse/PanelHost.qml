import QtQuick
import Nyx.Ui

// An app's mini-app panel in the preview area: Glimpse's own QML from panels/<id>/,
// loaded when its result is selected and destroyed when it isn't. The panel sees only
// this host:
//
// - `host.query`: the words typed after the app's name (`jazz` in `music jazz`);
// - `host.topic(name)`: the app's live state for one of its panel topics, as
//   `{state}`, `{loading}` or `{error}`; followed only while the panel exists;
// - `host.run(action, arguments, done)`: one of the app's own actions, checked as any
//   other call; one that asks first waits for the person's go-ahead here;
// - `host.open(arguments)`: the full app, at `arguments` when the app can open there;
// - `host.say(text)`, and `host.back()` to give the keyboard back to the search.
FocusScope {
    id: host

    required property var glimpse
    required property var backend
    // The selected result's `panel`: {id, app, name, icon, query, topics, size}.
    required property var offer

    readonly property string query: offer ? (offer.query || "") : ""
    readonly property string appId: offer ? offer.app : ""
    // A call that asks first, waiting for Enter or the button.
    property var pending: null

    signal back
    // The panel itself, for tests (GLIMPSE_TEST_IPC).
    readonly property var body: panelLoader.item

    // From the offer itself: on a switch to another app's panel, onOfferChanged can run
    // before appId follows.
    function topicName(name) {
        return "panel:" + (offer ? offer.app : "") + ":" + name;
    }
    function topic(name) {
        return backend.state[topicName(name)] || { loading: true };
    }
    function say(text) {
        glimpse.say(text);
    }
    function run(action, args, done, confirmed) {
        backend.request("PanelRun", { app: appId, action: action, arguments: args || {}, confirmed: confirmed === true }, (ok, err) => {
            if (err) {
                say(err.msg);
                if (done)
                    done(null, err.msg);
            } else if (ok.ask) {
                host.pending = { action: action, args: args || {}, done: done, message: ok.ask };
            } else if (done) {
                done(ok.result, null);
            }
        });
    }
    function confirm() {
        const p = pending;
        pending = null;
        if (p)
            run(p.action, p.args, p.done, true);
    }
    function open(args) {
        backend.request("PanelOpen", { app: appId, arguments: args || {} }, (ok, err) => {
            if (err)
                say(err.msg);
        });
    }

    // The topics are followed while the panel is shown, and let go after.
    property var watched: []
    // Each new search sends the offer again; the same topics stay watched.
    function watchAll() {
        const want = (offer && offer.topics ? offer.topics : []).map(t => topicName(t));
                if (want.join("\n") === watched.join("\n"))
            return;
        unwatchAll();
        watched = want;
        watched.forEach(t => backend.watch(t));
    }
    function unwatchAll() {
        watched.forEach(t => backend.unwatch(t));
        watched = [];
    }
    onOfferChanged: watchAll()
    Component.onCompleted: {
        watchAll();
        loadPanel();
    }
    Component.onDestruction: unwatchAll()

    readonly property var states: watched.map(t => backend.state[t] || { loading: true })
    readonly property var failed: states.find(s => s.error) || null
    readonly property bool loading: states.some(s => s.loading)

    Keys.onEscapePressed: e => {
        if (pending) {
            pending = null;
        } else {
            host.back();
        }
        e.accepted = true;
    }
    Keys.onReturnPressed: e => {
        if (pending) {
            confirm();
            e.accepted = true;
        } else {
            e.accepted = false;
        }
    }

    // The panel gets the host before it completes, so its first steps can use it.
    readonly property string panelId: offer ? offer.id : ""
    onPanelIdChanged: loadPanel()
    function loadPanel() {
        if (panelId === "")
            panelLoader.source = "";
        else
            panelLoader.setSource(Qt.resolvedUrl("panels/" + panelId + "/Panel.qml"), { host: host });
    }
    Loader {
        id: panelLoader
        anchors.fill: parent
        focus: true
        asynchronous: true
        visible: status === Loader.Ready && !host.failed
    }

    StateView {
        anchors.centerIn: parent
        width: parent.width
        visible: host.failed !== null || panelLoader.status !== Loader.Ready || (host.loading && host.states.length > 0 && !host.states.some(s => s.state !== undefined))
        kind: host.failed || panelLoader.status === Loader.Error ? "error" : "loading"
        text: host.failed ? host.failed.error : panelLoader.status === Loader.Error ? "This panel couldn't load" : ""
    }

    // What a call that asks first will do, over the panel.
    Rectangle {
        visible: host.pending !== null
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: ask.implicitHeight + 24
        radius: Style.radiusCard
        color: Style.card
        border.width: 1
        border.color: Style.alpha(Style.warning, 0.5)
        Row {
            id: ask
            anchors.verticalCenter: parent.verticalCenter
            anchors.left: parent.left
            anchors.leftMargin: 14
            anchors.right: parent.right
            anchors.rightMargin: 14
            spacing: 10
            Icon {
                anchors.verticalCenter: parent.verticalCenter
                name: "dialog-warning-symbolic"
                size: 18
                recolor: true
                color: Style.warning
            }
            Label {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width - 18 - goAhead.width - 20
                text: host.pending ? host.pending.message : ""
                wrapMode: Text.Wrap
            }
            Button {
                id: goAhead
                anchors.verticalCenter: parent.verticalCenter
                text: "Go ahead"
                primary: true
                onClicked: host.confirm()
            }
        }
    }
}
