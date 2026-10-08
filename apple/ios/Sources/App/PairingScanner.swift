import SwiftUI
import AVFoundation
import UIKit

struct PairingScanner: View {
    let onCode: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var error: String?

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()
                CameraScanner(onCode: onCode, onError: { error = $0 }).ignoresSafeArea()
                VStack(spacing: 24) {
                    Spacer()
                    Image(systemName: "viewfinder").font(.system(size: 180, weight: .ultraLight))
                    Text(error ?? "Point your camera at the QR in Pastazzo on your Mac.")
                        .multilineTextAlignment(.center).padding(20)
                        .background(.black.opacity(0.7), in: RoundedRectangle(cornerRadius: 16))
                    Spacer()
                }.foregroundStyle(.white).padding(24)
            }
            .navigationTitle("Scan Mac QR").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }
    }
}

private struct CameraScanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void
    let onError: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        ScannerController(onCode: onCode, onError: onError)
    }
    func updateUIViewController(_ controller: ScannerController, context: Context) {}
    static func dismantleUIViewController(_ controller: ScannerController, coordinator: ()) { controller.stop() }
}

private final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "org.pastazzo.camera")
    private let onCode: (String) -> Void
    private let onError: (String) -> Void
    private var preview: AVCaptureVideoPreviewLayer?
    private var finished = false
    private var stopped = false

    init(onCode: @escaping (String) -> Void, onError: @escaping (String) -> Void) {
        self.onCode = onCode; self.onError = onError
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }

    override func viewDidLoad() {
        super.viewDidLoad()
        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        self.preview = preview
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: configure()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .video) { granted in
                if granted { self.configure() }
                else { DispatchQueue.main.async { self.onError("Allow camera access in iPhone Settings → Pastazzo to scan your Mac QR.") } }
            }
        default: onError("Allow camera access in iPhone Settings → Pastazzo to scan your Mac QR.")
        }
    }
    override func viewDidLayoutSubviews() { super.viewDidLayoutSubviews(); preview?.frame = view.bounds }

    private func configure() {
        queue.async {
            guard !self.stopped else { return }
            do {
                guard let camera = AVCaptureDevice.default(for: .video) else {
                    throw NSError(domain: "Pastazzo", code: 1, userInfo: [NSLocalizedDescriptionKey: "No camera is available on this device."])
                }
                let input = try AVCaptureDeviceInput(device: camera)
                let output = AVCaptureMetadataOutput()
                guard self.session.canAddInput(input), self.session.canAddOutput(output) else {
                    throw NSError(domain: "Pastazzo", code: 2, userInfo: [NSLocalizedDescriptionKey: "The camera couldn't start. Try again."])
                }
                self.session.addInput(input); self.session.addOutput(output)
                output.setMetadataObjectsDelegate(self, queue: .main)
                output.metadataObjectTypes = [.qr]
                self.session.startRunning()
            } catch { DispatchQueue.main.async { self.onError(error.localizedDescription) } }
        }
    }
    func stop() { queue.async { self.stopped = true; self.session.stopRunning() } }
    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !finished, let link = objects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue })
            .first(where: { $0.hasPrefix("pastazzo://pair?") }) else { return }
        finished = true
        stop()
        onCode(link)
    }
}
