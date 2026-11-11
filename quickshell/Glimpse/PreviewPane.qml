import QtQuick
import Nyx.Ui

// The right-hand pane (the GTK panel's): a framed area, then the details. Every kind of
// result uses the same layout, so moving through results never reshuffles it. A
// calculation shows the calculator instead. The backend builds each preview; until it
// comes, the result's icon holds the frame, dimmed.
Item {
    id: pane

    required property var glimpse
    required property var backend
    property int cursor: 0
    property int selectionStart: 0
    property int selectionEnd: 0
    property string searchText: ""
    readonly property bool calc: glimpse.current && glimpse.results.calc !== null && glimpse.results.calc !== undefined
    readonly property bool hasMedia: body.type === "media" && !calc
    readonly property bool playing: media.item ? media.item.playing : false

    signal typed(string text, int cursor)
    signal refocus

    // In the stacked view, the app's panel stays while its results are browsed, and a
    // command's result takes its place; otherwise a panel shows while its row is selected.
    property var appOffer: null
    readonly property bool resultShown: glimpse.ranId !== ""
    readonly property var selectedRow: glimpse.rows.find(x => x.id === glimpse.selectedId) || null
    readonly property var rowOffer: selectedRow && selectedRow.panel && selectedRow.panel.available ? selectedRow.panel : null
    readonly property var shownOffer: appOffer ? (resultShown ? null : appOffer) : rowOffer
    // While the next search is on its way, the last one's: typing on doesn't rebuild it.
    property var heldOffer: null
    readonly property var panelOffer: glimpse.current ? shownOffer : heldOffer
    onShownOfferChanged: if (glimpse.current)
        heldOffer = shownOffer
    function panelCall(fn) {
        const p = miniApp.item ? miniApp.item.body : null;
        if (p && typeof p[fn] === "function")
            p[fn]();
    }
    function focusPanel() {
        if (miniApp.item)
            miniApp.item.forceActiveFocus();
    }

    function togglePlayback() {
        if (media.item)
            media.item.toggle();
    }

    // What the backend sent for the selected result.
    property var body: ({ type: "card", icon: "", title: "", desc: "", dim: true })
    property var details: []
    property int request: 0

    function load() {
        const r = glimpse.rows.find(x => x.id === glimpse.selectedId) || null;
        request += 1;
        if (!r || calc) {
            body = { type: "card", icon: "system-search-symbolic", title: "", desc: "", dim: true };
            details = [];
            return;
        }
        body = { type: "card", icon: r.icon, title: "", desc: "", dim: true };
        details = [];
        const mine = request;
        backend.request("Preview", { id: r.id, generation: glimpse.generation }, (ok, err) => {
            if (mine !== pane.request || err || !ok)
                return;
            pane.body = ok.body || { type: "card", icon: r.icon, title: "", desc: "" };
            pane.details = ok.details || [];
        });
    }
    Connections {
        target: pane.glimpse
        function onSelectedIdChanged() {
            pane.load();
        }
        function onPreviewSerialChanged() {
            pane.load();
        }
        function onCurrentChanged() {
            if (pane.glimpse.current)
                pane.heldOffer = pane.shownOffer;
        }
    }
    onCalcChanged: load()

    Loader {
        id: miniApp
        active: pane.panelOffer !== null && pane.glimpse.asked
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 16
        anchors.topMargin: 4
        anchors.bottomMargin: 14
        sourceComponent: PanelHost {
            glimpse: pane.glimpse
            backend: pane.backend
            offer: pane.panelOffer
            onBack: pane.refocus()
        }
    }

    Column {
        visible: !pane.calc && !miniApp.active
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 16
        anchors.topMargin: 4
        anchors.bottomMargin: 14
        spacing: 14

        Rectangle {
            id: frame
            width: parent.width
            height: parent.height - (detailGrid.visible ? detailGrid.height + parent.spacing : 0)
            radius: Style.radiusCard
            color: Style.card
            clip: true

            // A large icon with an optional title and line of text.
            Column {
                visible: pane.body.type === "card"
                anchors.centerIn: parent
                width: parent.width - 32
                spacing: 10
                Icon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    name: pane.body.icon || ""
                    size: 64
                    recolor: (pane.body.icon || "").endsWith("-symbolic")
                    color: Style.dimText
                    opacity: pane.body.dim ? 0.3 : 1
                }
                Text {
                    width: parent.width
                    visible: text !== ""
                    text: pane.body.title || ""
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    color: Style.text
                    font.family: Style.font
                    font.pixelSize: 16
                    font.weight: Font.DemiBold
                }
                Text {
                    width: parent.width
                    visible: text !== ""
                    text: pane.body.desc || ""
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    color: Style.dimText
                    font.family: Style.font
                    font.pixelSize: 13
                }
            }

            // An app: its icon, name and kind, what it does, its categories, then the facts.
            Item {
                visible: pane.body.type === "app"
                anchors.fill: parent
                anchors.margins: 22
                anchors.bottomMargin: 18
                Column {
                    width: parent.width
                    spacing: 14
                    Row {
                        width: parent.width
                        spacing: 16
                        Icon {
                            name: pane.body.icon || ""
                            size: 64
                            recolor: (pane.body.icon || "").endsWith("-symbolic")
                        }
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - 80
                            spacing: 2
                            Text {
                                width: parent.width
                                text: pane.body.name || ""
                                elide: Text.ElideRight
                                color: Style.text
                                font.family: Style.font
                                font.pixelSize: 18
                                font.weight: Font.DemiBold
                            }
                            Text {
                                width: parent.width
                                text: pane.body.kind || ""
                                elide: Text.ElideRight
                                color: Style.dimText
                                font.family: Style.font
                                font.pointSize: 12.5 * 0.75
                            }
                        }
                    }
                    Text {
                        width: parent.width
                        visible: text !== ""
                        text: pane.body.desc || ""
                        wrapMode: Text.Wrap
                        maximumLineCount: 4
                        elide: Text.ElideRight
                        color: Style.text
                        font.family: Style.font
                        font.pixelSize: 13
                    }
                    Flow {
                        width: parent.width
                        visible: (pane.body.chips || []).length > 0
                        spacing: 6
                        Repeater {
                            model: pane.body.chips || []
                            delegate: Rectangle {
                                required property string modelData
                                width: chip.implicitWidth + 18
                                height: 22
                                radius: Style.radiusSmall
                                color: Style.alpha(Style.accent, 0.12)
                                Text {
                                    id: chip
                                    anchors.centerIn: parent
                                    text: parent.modelData
                                    color: Style.accent
                                    font.family: Style.font
                                    font.pointSize: 11.5 * 0.75
                                    font.weight: Font.Medium
                                }
                            }
                        }
                    }
                }
                Grid {
                    anchors.bottom: parent.bottom
                    width: parent.width
                    columns: 2
                    rowSpacing: 8
                    columnSpacing: 18
                    Repeater {
                        model: (pane.body.info || []).length * 2
                        delegate: Text {
                            required property int index
                            readonly property var pair: pane.body.info[Math.floor(index / 2)]
                            readonly property bool key: index % 2 === 0
                            width: key ? 72 : parent.width - 90
                            text: key ? pair[0] : pair[1]
                            elide: Text.ElideRight
                            color: key ? Style.tertiary : Style.dimText
                            font.family: !key && pair[0] === "Command" ? Style.mono : Style.font
                            font.pixelSize: !key && pair[0] === "Command" ? 11.5 : 12
                        }
                    }
                }
            }

            // The passage that matched, the file's start, or a plugin's text.
            Flickable {
                id: textView
                visible: pane.body.type === "text"
                anchors.fill: parent
                clip: true
                contentHeight: passage.height
                boundsBehavior: Flickable.StopAtBounds
                Text {
                    id: passage
                    width: textView.width
                    padding: 12
                    leftPadding: 14
                    rightPadding: 14
                    text: pane.body.type === "text" ? pane.body.html : ""
                    textFormat: Text.RichText
                    wrapMode: Text.WrapAtWordBoundaryOrAnywhere
                    color: Style.text
                    font.family: Style.font
                    font.pixelSize: 12
                    onTextChanged: textView.contentY = 0
                }
            }

            Image {
                visible: pane.body.type === "image"
                anchors.fill: parent
                anchors.margins: 10
                source: pane.body.type === "image" ? "file://" + pane.body.path : ""
                fillMode: Image.PreserveAspectFit
                asynchronous: true
                cache: false
                smooth: true
                mipmap: true
            }

            Loader {
                id: media
                anchors.fill: parent
                anchors.margins: 10
                active: pane.body.type === "media" && pane.glimpse.asked
                sourceComponent: MediaPreview {
                    path: pane.body.path
                    video: pane.body.video === true
                    icon: pane.body.icon || ""
                }
            }
        }

        Grid {
            id: detailGrid
            visible: pane.details.length > 0
            x: 4
            width: parent.width - 8
            columns: 2
            rowSpacing: 6
            columnSpacing: 18
            Repeater {
                model: pane.details.length * 2
                delegate: Text {
                    required property int index
                    readonly property var pair: pane.details[Math.floor(index / 2)]
                    readonly property bool key: index % 2 === 0
                    width: key ? Math.max(76, implicitWidth) : detailGrid.width - 94
                    text: key ? pair[0] : pair[1]
                    // A folder keeps its end; anything else keeps its start.
                    elide: !key && pair[0] === "Folder" ? Text.ElideLeft : Text.ElideRight
                    color: key ? Style.dimText : Style.text
                    font.family: Style.font
                    font.pixelSize: 12
                    font.features: { "tnum": 1 }
                }
            }
        }
    }

    Calculator {
        visible: pane.calc
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 16
        anchors.topMargin: 4
        anchors.bottomMargin: 14
        display: pane.calc ? pane.glimpse.results.calc : ""
        onKey: k => pane.press(k)
        onCopyAnswer: pane.backend.request("CopyAnswer", { text: pane.searchText }, (ok, err) => {
            if (ok && ok.message)
                pane.glimpse.say(ok.message);
            pane.refocus();
        })
    }

    // A keypad key edits the search at its cursor, as the GTK keypad does.
    function press(k) {
        const t = searchText;
        let s = Math.min(selectionStart, selectionEnd), e = Math.max(selectionStart, selectionEnd);
        if (k === "C") {
            typed("=", 1);
        } else if (k === "⌫") {
            if (e > s)
                typed(t.substring(0, s) + t.substring(e), s);
            else if (cursor > 0)
                typed(t.substring(0, cursor - 1) + t.substring(cursor), cursor - 1);
            else
                refocus();
        } else if (k === "=") {
            refocus();
        } else {
            if (e <= s)
                s = e = cursor;
            typed(t.substring(0, s) + k + t.substring(e), s + k.length);
        }
    }
}
