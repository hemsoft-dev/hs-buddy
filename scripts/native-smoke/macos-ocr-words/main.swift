// Compiled headless OCR of a supplied image file. No window capture or input APIs.
import Foundation
import ImageIO
import Vision

struct Word: Encodable {
    let text: String
    let left: Double
    let top: Double
    let width: Double
    let height: Double
    let line: String
}

enum OcrFailure: Error { case invalidImage, invalidBounds }

do {
    guard CommandLine.arguments.count == 2 else { throw OcrFailure.invalidImage }
    let url = URL(fileURLWithPath: CommandLine.arguments[1])
    let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
    guard let size = attributes[.size] as? NSNumber,
          size.uint64Value <= 64 * 1024 * 1024,
          let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
          image.width > 0, image.height > 0,
          image.width <= 12000, image.height <= 12000,
          image.width * image.height <= 32 * 1024 * 1024
    else { throw OcrFailure.invalidImage }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.recognitionLanguages = ["en-US"]
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: image, options: [:]).perform([request])
    var words: [Word] = []
    for (line, observation) in (request.results ?? []).enumerated() {
        guard let candidate = observation.topCandidates(1).first else { continue }
        let text = candidate.string
        for token in text.split(whereSeparator: { $0.isWhitespace }) {
            guard let bounds = try candidate.boundingBox(for: token.startIndex..<token.endIndex) else {
                throw OcrFailure.invalidBounds
            }
            let box = bounds.boundingBox
            guard box.minX.isFinite, box.minY.isFinite,
                  box.width > 0, box.height > 0,
                  box.minX >= 0, box.minY >= 0, box.maxX <= 1, box.maxY <= 1
            else { throw OcrFailure.invalidBounds }
            words.append(Word(
                text: String(token),
                left: box.minX * Double(image.width),
                top: (1 - box.maxY) * Double(image.height),
                width: box.width * Double(image.width),
                height: box.height * Double(image.height),
                line: String(line)
            ))
        }
    }
    FileHandle.standardOutput.write(try JSONEncoder().encode(words))
    FileHandle.standardOutput.write(Data([10]))
} catch {
    FileHandle.standardError.write(Data("Native image OCR failed.\n".utf8))
    exit(1)
}
