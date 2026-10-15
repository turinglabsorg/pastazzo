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
        !saving && (!(contentText ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || providers.contains {
            $0.hasItemConformingToTypeIdentifier(UTType.image.identifier) || $0.hasItemConformingToTypeIdentifier(UTType.url.identifier)
        })
    }

    private var providers: [NSItemProvider] {
        (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
    }

    override func didSelectPost() {
        guard !saving else { return }
        saving = true
        title = "Sending securely…"
        validateContent()
        Task { @MainActor in
            defer { saving = false; title = "Save to Pastazzo"; validateContent() }
            do {
                let copy = try await ClipboardImport.load(providers, text: contentText ?? "", maxImageDimension: 2048)
                await send(copy)
            } catch {
                let alert = UIAlertController(title: "Couldn't save", message: error.localizedDescription, preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                present(alert, animated: true)
            }
        }
    }

    private func send(_ copy: SharedCopy) async {
        saving = true
        title = "Sending securely…"
        validateContent()
        defer { saving = false; title = "Save to Pastazzo"; validateContent() }
        do {
            let outcome = try await SharedSender.send(copy)
            let alert = UIAlertController(title: outcome.title, message: outcome.message, preferredStyle: .alert)
            if outcome == .queued {
                alert.addAction(UIAlertAction(title: "Retry now", style: .default) { _ in
                    Task { @MainActor in await self.send(copy) }
                })
            }
            alert.addAction(UIAlertAction(title: "Done", style: .default) { _ in self.extensionContext?.completeRequest(returningItems: [], completionHandler: nil) })
            present(alert, animated: true)
        } catch {
            let alert = UIAlertController(title: "Couldn't save", message: error.localizedDescription, preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
        }
    }

    override func configurationItems() -> [Any]! { [] }
}
