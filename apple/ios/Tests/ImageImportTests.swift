import XCTest
import UniformTypeIdentifiers
import ImageIO
@testable import Pastazzo

final class ImageImportTests: XCTestCase {
    private func image() -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        return UIGraphicsImageRenderer(size: CGSize(width: 32, height: 24), format: format).image { context in
            UIColor.orange.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 32, height: 24))
        }
    }

    private func provider(data: Data, type: UTType) -> NSItemProvider {
        let provider = NSItemProvider()
        provider.registerDataRepresentation(forTypeIdentifier: type.identifier, visibility: .all) { completion in
            completion(data, nil)
            return nil
        }
        return provider
    }

    @MainActor
    func testPasteButtonProviderImportsImageBeforeAccompanyingText() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let model = HistoryModel(client: try MobileClient(root: root))
        let photo = NSItemProvider(object: image())
        let caption = NSItemProvider(object: "Photo caption" as NSString)
        await model.paste([caption, photo])
        XCTAssertEqual(model.items.count, 1, model.message ?? "")
        XCTAssertEqual(model.items.first?.kind, "image")
        let item = try XCTUnwrap(model.items.first)
        let result = try await model.client.call("item", ["id": item.id])
        let content = try XCTUnwrap(result["item"] as? [String: Any])
        XCTAssertEqual(content["mime"] as? String, "image/png")
        let bytes = try XCTUnwrap(HistoryModel.decodeBase64URL(try XCTUnwrap(content["data"] as? String)))
        XCTAssertEqual(UIImage(data: bytes)?.size, CGSize(width: 32, height: 24))
        await model.paste([photo])
        XCTAssertEqual(model.items.count, 1)
    }

    @MainActor
    func testShareFilePhotoSurvivesRestartAndImportsOnce() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let file = root.appendingPathComponent("photo.jpg")
        try XCTUnwrap(image().jpegData(compressionQuality: 0.9)).write(to: file)
        let provider = NSItemProvider()
        provider.registerFileRepresentation(forTypeIdentifier: UTType.jpeg.identifier, fileOptions: [], visibility: .all) { completion in
            completion(file, false, nil)
            return nil
        }
        let copy = try await ClipboardImport.load([provider], text: "A photo from Photos")
        XCTAssertEqual(copy.kind, "image")
        XCTAssertEqual(copy.mime, "image/png")
        let inbox = root.appendingPathComponent("Inbox")
        try SharedInbox.save(copy, to: inbox)
        let model = HistoryModel(client: try MobileClient(root: root.appendingPathComponent("App")))
        await model.importShared(from: inbox)
        XCTAssertEqual(model.items.count, 1)
        XCTAssertEqual(model.items[0].kind, "image")
        try SharedInbox.save(copy, to: inbox)
        let restarted = HistoryModel(client: try MobileClient(root: model.client.root))
        await restarted.importShared(from: inbox)
        XCTAssertEqual(restarted.items.count, 1)
        XCTAssertTrue(try FileManager.default.contentsOfDirectory(atPath: inbox.path).isEmpty)
    }

    func testHEICDataAndOrientationAreConvertedToPortablePNG() async throws {
        let bytes = NSMutableData()
        let destination = try XCTUnwrap(CGImageDestinationCreateWithData(bytes, UTType.heic.identifier as CFString, 1, nil))
        CGImageDestinationAddImage(destination, try XCTUnwrap(image().cgImage), [kCGImagePropertyOrientation: 6] as CFDictionary)
        XCTAssertTrue(CGImageDestinationFinalize(destination))
        let copy = try await ClipboardImport.load([provider(data: bytes as Data, type: .heic)])
        let png = try XCTUnwrap(copy.data)
        XCTAssertEqual(Array(png.prefix(8)), [137, 80, 78, 71, 13, 10, 26, 10])
        XCTAssertEqual(UIImage(data: png)?.size, CGSize(width: 24, height: 32))
    }

    func testInvalidImageDoesNotBecomeCaptionText() async throws {
        do {
            _ = try await ClipboardImport.load([provider(data: Data("not an image".utf8), type: .jpeg)], text: "caption")
            XCTFail("A broken photo must report an error without saving its caption instead")
        } catch { XCTAssertTrue(error.localizedDescription.contains("image")) }
        XCTAssertThrowsError(try ClipboardImport.pngData(Data(repeating: 0, count: 25 * 1024 * 1024 + 1)))
    }

    func testLargePhotoIsBoundedWithoutUpscalingOrChangingItsAspectRatio() throws {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let photo = UIGraphicsImageRenderer(size: CGSize(width: 6144, height: 64), format: format).image { context in
            UIColor.orange.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 6144, height: 64))
        }
        let png = try ClipboardImport.pngData(try XCTUnwrap(photo.jpegData(compressionQuality: 0.9)))
        XCTAssertEqual(UIImage(data: png)?.size, CGSize(width: 3072, height: 32))
        XCTAssertLessThanOrEqual(png.count, 25 * 1024 * 1024)
    }

    func testTextAndURLProvidersRemainSupported() async throws {
        let text = try await ClipboardImport.load([NSItemProvider(object: "Shared text" as NSString)])
        XCTAssertEqual(text.text, "Shared text")
        let url = NSURL(string: "https://example.org/photo")!
        let shared = try await ClipboardImport.load([NSItemProvider(object: url)], text: "Link")
        XCTAssertEqual(shared.text, "Link\nhttps://example.org/photo")
    }
}
