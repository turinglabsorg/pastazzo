import UIKit
import Social
import UniformTypeIdentifiers

final class ShareViewController: SLComposeServiceViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Save to Pastazzo"
        placeholder = "Shared text"
    }

    override func isContentValid() -> Bool {
        !(contentText ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || providers.contains {
            $0.hasItemConformingToTypeIdentifier(UTType.image.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.url.identifier)
        }
    }

    private var providers: [NSItemProvider] {
        (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
    }

    override func didSelectPost() {
        Task { @MainActor in
            do {
                let id = UUID().uuidString
                if let provider = providers.first(where: { $0.canLoadObject(ofClass: UIImage.self) }) {
                    let image: UIImage = try await withCheckedThrowingContinuation { continuation in
                        provider.loadObject(ofClass: UIImage.self) { object, error in
                            if let error { continuation.resume(throwing: error) }
                            else if let image = object as? UIImage { continuation.resume(returning: image) }
                            else { continuation.resume(throwing: SharedInbox.tooLarge) }
                        }
                    }
                    guard let data = image.pngData() else { throw SharedInbox.tooLarge }
                    try SharedInbox.save(SharedCopy(id: id, kind: "image", text: nil, data: data, mime: "image/png"))
                } else {
                    var text = (contentText ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                    if let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(UTType.url.identifier) }) {
                        let value: NSSecureCoding? = try await withCheckedThrowingContinuation { continuation in
                            provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { value, error in
                                if let error { continuation.resume(throwing: error) }
                                else { continuation.resume(returning: value) }
                            }
                        }
                        if let url = value as? URL { text = text.isEmpty ? url.absoluteString : "\(text)\n\(url.absoluteString)" }
                    }
                    guard !text.isEmpty else { throw MobileShareError.empty }
                    try SharedInbox.save(SharedCopy(id: id, kind: "text", text: text, data: nil, mime: nil))
                }
                let alert = UIAlertController(title: "Saved", message: "Open Pastazzo to sync this copy with your devices.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "Done", style: .default) { _ in self.extensionContext?.completeRequest(returningItems: [], completionHandler: nil) })
                present(alert, animated: true)
            } catch {
                let alert = UIAlertController(title: "Couldn't save", message: error.localizedDescription, preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                present(alert, animated: true)
            }
        }
    }

    override func configurationItems() -> [Any]! { [] }
}

private enum MobileShareError: LocalizedError {
    case empty
    var errorDescription: String? { "No text or supported image was shared." }
}
