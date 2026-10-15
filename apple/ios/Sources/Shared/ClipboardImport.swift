import UIKit
import UniformTypeIdentifiers
import ImageIO

enum ClipboardImport {
    static let supportedTypes: [UTType] = [.image, .url, .plainText]
    private static let imageLimit = 25 * 1024 * 1024

    static func load(_ providers: [NSItemProvider], text: String = "", maxImageDimension: Int = 3072) async throws -> SharedCopy {
        let id = UUID().uuidString
        if let provider = providers.first(where: { $0.hasItemConformingToTypeIdentifier(UTType.image.identifier) }) {
            let data = try await imageData(from: provider, maxDimension: maxImageDimension)
            return SharedCopy(id: id, kind: "image", text: nil, data: data, mime: "image/png")
        }
        var content = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if let provider = providers.first(where: { $0.canLoadObject(ofClass: NSURL.self) }) {
            let url: NSURL = try await object(from: provider, as: NSURL.self)
            if content.isEmpty { content = url.absoluteString ?? "" }
            else if content != url.absoluteString { content += "\n" + (url.absoluteString ?? "") }
        } else if content.isEmpty, let provider = providers.first(where: { $0.canLoadObject(ofClass: NSString.self) }) {
            let value: NSString = try await object(from: provider, as: NSString.self)
            content = value as String
        }
        guard !content.isEmpty else { throw ImportError.empty }
        guard content.utf8.count <= 1024 * 1024 else { throw SharedInbox.tooLarge }
        return SharedCopy(id: id, kind: "text", text: content, data: nil, mime: nil)
    }

    static func pngData(_ data: Data, maxDimension: Int = 3072) throws -> Data {
        guard data.count <= imageLimit else { throw SharedInbox.tooLarge }
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { throw ImportError.image }
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        let width = properties?[kCGImagePropertyPixelWidth] as? Int ?? 0
        let height = properties?[kCGImagePropertyPixelHeight] as? Int ?? 0
        guard width > 0, height > 0 else { throw ImportError.image }
        let maxSize = min(max(1, maxDimension), max(width, height))
        var options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxSize,
            kCGImageSourceShouldCacheImmediately: true
        ]
        var png: Data?
        for size in [maxSize, min(maxSize, 2048)] {
            options[kCGImageSourceThumbnailMaxPixelSize] = size
            png = autoreleasepool {
                guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
                return UIImage(cgImage: image).pngData()
            }
            if let png, png.count <= imageLimit { return png }
        }
        guard let png else { throw ImportError.image }
        guard png.count <= imageLimit else { throw SharedInbox.tooLarge }
        return png
    }

    private static func imageData(from provider: NSItemProvider, maxDimension: Int) async throws -> Data {
        let types = provider.registeredTypeIdentifiers.filter { UTType($0)?.conforms(to: .image) == true }
        var failure: Error = ImportError.image
        for type in types {
            do {
                let data: Data = try await withCheckedThrowingContinuation { continuation in
                    provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
                        do {
                            guard let url else { throw error ?? ImportError.image }
                            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
                            guard size <= imageLimit else { throw SharedInbox.tooLarge }
                            continuation.resume(returning: try Data(contentsOf: url, options: .mappedIfSafe))
                        } catch { continuation.resume(throwing: error) }
                    }
                }
                return try pngData(data, maxDimension: maxDimension)
            } catch { failure = error }
            do {
                let data: Data = try await withCheckedThrowingContinuation { continuation in
                    provider.loadDataRepresentation(forTypeIdentifier: type) { data, error in
                        if let data { continuation.resume(returning: data) }
                        else { continuation.resume(throwing: error ?? ImportError.image) }
                    }
                }
                return try pngData(data, maxDimension: maxDimension)
            } catch { failure = error }
        }
        throw failure
    }

    private static func object<T: NSItemProviderReading>(from provider: NSItemProvider, as type: T.Type) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            provider.loadObject(ofClass: type) { object, error in
                if let object = object as? T { continuation.resume(returning: object) }
                else { continuation.resume(throwing: error ?? ImportError.empty) }
            }
        }
    }

    private enum ImportError: LocalizedError {
        case empty, image
        var errorDescription: String? {
            switch self {
            case .empty: return "No text or supported image was provided."
            case .image: return "This image couldn't be read. Try sharing the photo as an image."
            }
        }
    }
}
