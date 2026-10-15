import UIKit
import Social
import UniformTypeIdentifiers

final class ShareViewController: SLComposeServiceViewController {
    private var saving = false
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
        guard !saving else { return }
        saving = true
        Task { @MainActor in
            defer { saving = false }
            do {
                let copy = try await ClipboardImport.load(providers, text: contentText ?? "")
                try SharedInbox.save(copy)
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
