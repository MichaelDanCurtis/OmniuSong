import AppKit
import WebKit
import CoreMIDI

final class StudioDelegate: NSObject, NSApplicationDelegate, WKScriptMessageHandler, WKNavigationDelegate, WKUIDelegate {
    var window: NSWindow!
    var webView: WKWebView!
    var midiClient = MIDIClientRef()
    var midiPort = MIDIPortRef()
    let studioURL = URL(string: "http://127.0.0.1:43178")!

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        let menu = NSMenu()
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "About OmniuSong", action: #selector(about), keyEquivalent: "")
        appMenu.addItem(.separator())
        appMenu.addItem(withTitle: "Quit OmniuSong", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appMenuItem.submenu = appMenu
        menu.addItem(appMenuItem)
        let editItem = NSMenuItem()
        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = edit; menu.addItem(editItem); NSApp.mainMenu = menu

        let config = WKWebViewConfiguration()
        config.userContentController.add(self, name: "omniu")
        config.preferences.setValue(true, forKey: "javaScriptCanOpenWindowsAutomatically")
        webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = self
        webView.uiDelegate = self
        if #available(macOS 13.3, *) { webView.isInspectable = true }
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1440, height: 1000), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "OmniuSong — Studio"
        window.minSize = NSSize(width: 800, height: 650)
        window.contentView = webView
        window.center(); window.makeKeyAndOrderFront(nil)
        webView.load(URLRequest(url: studioURL))
        NSApp.activate(ignoringOtherApps: true)
    }
    @objc func about() {
        let alert = NSAlert(); alert.messageText = "OmniuSong 0.1"
        alert.informativeText = "Local development preview. MIDI 2.0 UMP input uses CoreMIDI. Audio playback uses the studio web audio engine. Start the local server before opening this app."
        alert.runModal()
    }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame,
              let url = message.frameInfo.request.url,
              url.host == studioURL.host, url.port == studioURL.port,
              let payload = message.body as? [String: Any],
              let action = payload["action"] as? String else { return }
        if action == "connectMIDI" { connectMIDI() }
    }
    func emit(_ payload: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: payload), let json = String(data: data, encoding: .utf8) else { return }
        webView.evaluateJavaScript("window.omniuNative && window.omniuNative(\(json));", completionHandler: nil)
    }
    func connectMIDI() {
        if midiClient == 0 {
            let status = MIDIClientCreateWithBlock("OmniuSong" as CFString, &midiClient) { [weak self] _ in
                DispatchQueue.main.async { self?.connectMIDI() }
            }
            guard status == noErr else { emit(["type": "status", "message": "CoreMIDI client error \(status)"]); return }
        }
        if midiPort != 0 { MIDIPortDispose(midiPort); midiPort = 0 }
        let status = MIDIInputPortCreateWithProtocol(midiClient, "OmniuSong UMP input" as CFString, ._2_0, &midiPort) { [weak self] events, _ in
            let offset = MemoryLayout<MIDIEventList>.offset(of: \MIDIEventList.packet)!
            var packet = UnsafeRawPointer(events).advanced(by: offset).assumingMemoryBound(to: MIDIEventPacket.self)
            for _ in 0..<events.pointee.numPackets {
                let count = Int(packet.pointee.wordCount)
                if count > 0 && count <= 65536 {
                    let wordsOffset = MemoryLayout<MIDIEventPacket>.offset(of: \MIDIEventPacket.words)!
                    let rawWords = UnsafeRawPointer(packet).advanced(by: wordsOffset).assumingMemoryBound(to: UInt32.self)
                    let words = Array(UnsafeBufferPointer(start: rawWords, count: count))
                    DispatchQueue.main.async { self?.emit(["type": "ump", "words": words]) }
                }
                packet = UnsafePointer(MIDIEventPacketNext(packet))
            }
        }
        guard status == noErr else { emit(["type": "status", "message": "CoreMIDI port error \(status)"]); return }
        var connected = 0
        for i in 0..<MIDIGetNumberOfSources() {
            let source = MIDIGetSource(i)
            if MIDIPortConnectSource(midiPort, source, nil) == noErr { connected += 1 }
        }
        emit(["type": "status", "message": "\(connected) CoreMIDI input(s) · UMP protocol 2.0"])
    }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else { decisionHandler(.cancel); return }
        if url.host == studioURL.host && url.port == studioURL.port && url.scheme == "http" { decisionHandler(.allow) }
        else { if url.scheme == "https" { NSWorkspace.shared.open(url) }; decisionHandler(.cancel) }
    }
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url, url.scheme == "https" { NSWorkspace.shared.open(url) }; return nil
    }
    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin, initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType, decisionHandler: @escaping (WKPermissionDecision) -> Void) {
        decisionHandler(origin.host == "127.0.0.1" && origin.port == 43178 ? .prompt : .deny)
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        let alert = NSAlert(); alert.messageText = "Start the OmniuSong studio server"
        alert.informativeText = "Run scripts/start-mac.command in the OmniuSong project, then open the app again. The development app connects only to 127.0.0.1:43178."
        alert.runModal()
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func applicationWillTerminate(_ notification: Notification) {
        if midiPort != 0 { MIDIPortDispose(midiPort) }
        if midiClient != 0 { MIDIClientDispose(midiClient) }
    }
}

let app = NSApplication.shared
let delegate = StudioDelegate()
app.delegate = delegate
app.run()
