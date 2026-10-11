import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import qs.Commons
import qs.Ui

Item {
  id: root

  property var shell: null
  property var manifest: null

  property bool opened: false
  property string filterText: ""
  property int selectedIndex: 0
  property bool cursorActive: false
  property int pendingIndex: -1

  readonly property string home: Quickshell.env("HOME")
  readonly property string pluginDir: home + "/.config/omarchy/plugins/turinglabs.pastazzo"
  readonly property string bin: home + "/.local/bin/pastazzo"
  readonly property string captureScript: pluginDir + "/capture.sh"
  readonly property string pasteScript: pluginDir + "/paste.sh"

  property color background: Color.menu.background
  property color foreground: Color.menu.text
  property color border: Color.menu.border
  property var borderSpec: Border.surfaceSpec("menu", "border", border, Math.max(1, Style.space(2)))
  property color scrim: Color.menu.scrim
  property color selectedBackground: Color.menu.selectedBackground
  property color selectedText: Color.menu.selectedText
  readonly property int cornerRadius: Style.cornerRadius
  property string fontFamily: Style.font.menuFamily
  property string localDevice: ""
  property int contentMargin: Style.spacing.panelPadding
  property int headerHeight: Math.max(Style.space(34), Style.font.title + Style.spacing.controlPaddingY * 2)
  property int contentSpacing: Style.spacing.md
  property int rowHeight: Math.max(Style.space(52), Style.font.body * 2 + Style.spacing.sm)
  property int cardWidth: Math.min(Style.space(560), panel.width - Style.gapsOut * 2)
  property int cardHeight: Math.min(Style.space(520), panel.height - Style.gapsOut * 2)

  function open(payloadJson) {
    root.opened = true
    root.filterText = ""
    root.selectedIndex = 0
    root.cursorActive = true
    root.pendingIndex = -1
    clickTimer.stop()
    root.search()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function close() {
    root.opened = false
    clickTimer.stop()
  }

  function sourceName(origin) {
    var name = String(origin || "")
    return name !== "" ? name : root.localDevice
  }

  function dismiss() {
    root.close()
    if (root.shell && typeof root.shell.hide === "function")
      root.shell.hide((root.manifest && root.manifest.id) || "turinglabs.pastazzo")
  }

  function toggle() {
    if (root.opened) root.dismiss()
    else root.open("{}")
  }

  function setFilter(text) {
    root.filterText = text
    root.selectedIndex = 0
    searchTimer.restart()
  }

  function search() {
    searchProc.running = false
    searchProc.command = [root.bin, "search", root.filterText]
    searchProc.running = true
  }

  function loadResults(raw) {
    displayModel.clear()
    var items = []
    try {
      items = JSON.parse(raw || "[]")
    } catch (error) {
      items = []
    }
    if (!Array.isArray(items)) items = []
    for (var i = 0; i < items.length; i++) {
      var item = items[i]
      displayModel.append({
        itemId: String(item.id || ""),
        kind: String(item.kind || "text"),
        mime: String(item.mime || "text/plain"),
        preview: String(item.preview || ""),
        origin: String(item.origin || ""),
        path: String(item.path || "")
      })
    }
    if (displayModel.count === 0) selectedIndex = 0
    else if (selectedIndex >= displayModel.count) selectedIndex = displayModel.count - 1
    cursorActive = displayModel.count > 0
  }

  function select(delta) {
    if (displayModel.count === 0) return
    if (!cursorActive) {
      cursorActive = true
      selectedIndex = delta < 0 ? displayModel.count - 1 : 0
    } else {
      selectedIndex = (selectedIndex + delta + displayModel.count) % displayModel.count
    }
    resultList.positionViewAtIndex(selectedIndex, ListView.Contain)
  }

  function applyIndex(index, mode) {
    if (index < 0 || index >= displayModel.count) return
    var row = displayModel.get(index)
    if (!row || !row.itemId) return
    root.dismiss()
    Quickshell.execDetached([root.pasteScript, row.itemId, row.kind, row.mime, mode])
  }

  function queueActivate(index) {
    root.cursorActive = true
    root.selectedIndex = index
    if (clickTimer.running && root.pendingIndex === index) {
      clickTimer.stop()
      root.pendingIndex = -1
      root.applyIndex(index, "paste")
      return
    }
    root.pendingIndex = index
    clickTimer.restart()
  }

  FileView {
    id: syncFile
    path: root.home + "/.config/pastazzo/sync.json"
    watchChanges: true
    printErrors: false
    onLoaded: {
      try {
        root.localDevice = String(JSON.parse(text()).device_name || "")
      } catch (error) {
        root.localDevice = ""
      }
    }
  }

  ListModel { id: displayModel }

  Timer {
    id: searchTimer
    interval: 80
    onTriggered: root.search()
  }

  Timer {
    id: clickTimer
    interval: 250
    onTriggered: {
      var index = root.pendingIndex
      root.pendingIndex = -1
      root.applyIndex(index, "copy")
    }
  }

  Process {
    id: searchProc
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.loadResults(text)
    }
  }

  Process {
    id: initProc
    command: ["pkill", "-f", "wl-paste .*turinglabs.pastazzo/capture\\.sh"]
    onExited: {
      textWatch.running = true
      imageWatch.running = true
    }
  }

  Process {
    id: textWatch
    command: ["setpriv", "--pdeathsig", "TERM", "wl-paste", "--type", "text", "--watch", root.captureScript, "text"]
    onExited: watchRestart.restart()
  }

  Process {
    id: imageWatch
    command: ["setpriv", "--pdeathsig", "TERM", "wl-paste", "--type", "image/png", "--watch", root.captureScript, "image/png"]
    onExited: watchRestart.restart()
  }

  Timer {
    id: watchRestart
    interval: 1500
    onTriggered: {
      if (!textWatch.running) textWatch.running = true
      if (!imageWatch.running) imageWatch.running = true
    }
  }

  Component.onCompleted: {
    syncFile.reload()
    initProc.running = true
  }

  PanelWindow {
    id: panel
    visible: root.opened
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"
    WlrLayershell.namespace: "turinglabs-pastazzo"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    exclusionMode: ExclusionMode.Ignore

    Rectangle {
      anchors.fill: parent
      color: root.scrim
    }

    MouseArea {
      anchors.fill: parent
      onClicked: root.dismiss()
    }

    BorderSurface {
      id: card
      width: root.cardWidth
      height: root.cardHeight
      radius: root.cornerRadius
      anchors.centerIn: parent
      color: root.background
      borderSpec: root.borderSpec
      padding: root.contentMargin

      MouseArea { anchors.fill: parent; onClicked: {} }

      Item {
        id: keyCatcher
        anchors.fill: parent
        focus: true
        Keys.priority: Keys.BeforeItem
        Keys.onPressed: function(event) {
          if (event.key === Qt.Key_Escape) {
            if (root.filterText) root.setFilter("")
            else root.dismiss()
            event.accepted = true
          } else if (event.key === Qt.Key_Up) {
            root.select(-1)
            event.accepted = true
          } else if (event.key === Qt.Key_Down) {
            root.select(1)
            event.accepted = true
          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            var mode = (event.modifiers & Qt.ShiftModifier) ? "paste" : "copy"
            root.applyIndex(root.selectedIndex, mode)
            event.accepted = true
          } else if (event.key === Qt.Key_Backspace) {
            root.setFilter(root.filterText.slice(0, -1))
            event.accepted = true
          } else if (event.text && event.text.length === 1 && event.text.charCodeAt(0) >= 32 && event.text.charCodeAt(0) !== 127) {
            root.setFilter(root.filterText + event.text)
            event.accepted = true
          }
        }
      }

      Column {
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset
        spacing: root.contentSpacing

        Text {
          width: parent.width
          height: root.headerHeight
          verticalAlignment: Text.AlignVCenter
          textFormat: Text.PlainText
          text: root.filterText || "Search clipboard…"
          color: root.foreground
          opacity: root.filterText ? 1 : 0.58
          font.family: root.fontFamily
          font.pixelSize: Style.font.heading
          elide: Text.ElideRight
        }

        ListView {
          id: resultList
          width: parent.width
          height: parent.height - root.headerHeight - root.contentSpacing
          model: displayModel
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          visible: displayModel.count > 0

          delegate: Rectangle {
            required property int index
            required property string itemId
            required property string kind
            required property string mime
            required property string preview
            required property string origin

            readonly property bool hasCursor: root.cursorActive && index === root.selectedIndex
            width: resultList.width
            height: root.rowHeight
            radius: root.cornerRadius
            color: hasCursor ? root.selectedBackground : "transparent"

            Row {
              anchors.fill: parent
              anchors.leftMargin: Style.spacing.md
              anchors.rightMargin: Style.spacing.md
              spacing: Style.spacing.md

              Column {
                width: parent.width - sourceLabel.width - parent.spacing
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2

                Text {
                  width: parent.width
                  textFormat: Text.PlainText
                  text: preview || (kind === "image" ? "Image" : "")
                  color: hasCursor ? root.selectedText : root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.body
                  elide: Text.ElideRight
                }

                Text {
                  width: parent.width
                  textFormat: Text.PlainText
                  text: kind === "image" ? (mime || "Image") : "Text"
                  color: hasCursor ? root.selectedText : root.foreground
                  opacity: 0.55
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                  elide: Text.ElideRight
                }
              }

              Text {
                id: sourceLabel
                anchors.verticalCenter: parent.verticalCenter
                visible: text !== ""
                textFormat: Text.PlainText
                text: root.sourceName(origin)
                color: hasCursor ? root.selectedText : root.foreground
                opacity: 0.9
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
                font.weight: Font.DemiBold
              }
            }

            MouseArea {
              anchors.fill: parent
              hoverEnabled: true
              onContainsMouseChanged: if (containsMouse) {
                root.cursorActive = true
                root.selectedIndex = index
              }
              onClicked: root.queueActivate(index)
            }
          }
        }

        Text {
          anchors.horizontalCenter: parent.horizontalCenter
          visible: displayModel.count === 0
          text: "Nothing copied yet"
          color: root.foreground
          opacity: 0.6
          font.family: root.fontFamily
          font.pixelSize: Style.font.body
        }
      }
    }
  }
}
