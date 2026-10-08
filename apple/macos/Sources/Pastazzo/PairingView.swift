import AppKit
import SwiftUI

struct PairingView: View {
    @ObservedObject var model: SettingsModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 16) {
            Text(model.pairingCompleted ? "Your iPhone is connected" : "Connect your iPhone").font(.title2.weight(.semibold))
            if model.pairingCompleted {
                Image(systemName: "checkmark.shield.fill").font(.system(size: 60)).foregroundColor(.green)
                Text("Your devices now share encrypted copies.").foregroundColor(.secondary)
            } else {
                Text("Open Pastazzo on your iPhone and tap Scan Mac QR.").foregroundColor(.secondary)
                if let image = model.pairingImage {
                    Image(nsImage: image).interpolation(.none).resizable().frame(width: 280, height: 280)
                        .padding(20).background(Color.white).cornerRadius(12)
                        .accessibilityLabel("Temporary Pastazzo pairing QR")
                    if let expiry = model.pairingExpiry {
                        Text("Expires at \(expiry, style: .time)").font(.caption).foregroundColor(.secondary)
                    }
                } else if model.pairingBusy { ProgressView("Preparing your QR…") }
                if let name = model.pairingName, let code = model.pairingCode {
                    Text("\(name) is ready to connect.").font(.headline)
                    Text(code).font(.system(.title3, design: .monospaced))
                    Button("Connect \(name)") { model.approvePairing() }.disabled(model.pairingBusy)
                        .keyboardShortcut(.defaultAction)
                }
                if let error = model.pairingError {
                    Text(error).foregroundColor(.red).fixedSize(horizontal: false, vertical: true)
                    Button("Generate New QR") { model.startPairing() }.disabled(model.pairingBusy)
                }
            }
            Button(model.pairingCompleted ? "Done" : "Cancel") { dismiss() }
        }
        .padding(24).frame(width: 410)
    }
}
