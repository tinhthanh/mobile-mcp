// Compact on-device OCR for Android screenshots (Apple Vision, macOS only).
// Compiled on first use by src/ocr.ts (needs Xcode command line tools).
// Usage: ocr <image> [fast]     env: LANGS=vi-VT,en-US  STATUSBAR=<px>
// Output: one "text|center_x,center_y" line per recognized text block.
import Foundation
import Vision
import AppKit

let args = CommandLine.arguments
let env = ProcessInfo.processInfo.environment
let req = VNRecognizeTextRequest()
let img = NSImage(contentsOfFile: args[1])!
var r = CGRect(origin: .zero, size: img.size)
let cg = img.cgImage(forProposedRect: &r, context: nil, hints: nil)!
let W = Double(cg.width), H = Double(cg.height)
req.recognitionLevel = args.count > 2 && args[2] == "fast" ? .fast : .accurate
req.recognitionLanguages = (env["LANGS"] ?? "vi-VT,en-US").split(separator: ",").map(String.init)
req.usesLanguageCorrection = true
try VNImageRequestHandler(cgImage: cg).perform([req])

// Status bar icons (signal, battery) OCR into garbage; skip that strip.
let statusBar = Double(env["STATUSBAR"] ?? "") ?? H * 0.06
for o in req.results ?? [] {
  guard let c = o.topCandidates(1).first else { continue }
  let b = o.boundingBox
  let x = b.midX * W, y = (1 - b.midY) * H, w = b.width * W, h = b.height * H
  if y < statusBar { continue }
  // Drop trailing chevrons ("Off >") and pure-symbol blocks (">", "*").
  var s = c.string.trimmingCharacters(in: .whitespaces)
  while s.hasSuffix(">") || s.hasSuffix("›") { s = String(s.dropLast()).trimmingCharacters(in: .whitespaces) }
  let alnum = s.unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) }.count
  if alnum == 0 { continue }
  // 1-2 char blocks in a near-square box are almost always icons read as letters ("JD", "Lo").
  if alnum <= 2 && w / h < 1.5 { continue }
  print("\(s)|\(Int(x)),\(Int(y))")
}
