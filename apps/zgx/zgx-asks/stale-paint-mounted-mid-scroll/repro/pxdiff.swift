import CoreGraphics
import ImageIO
import Foundation
func load(_ p: String) -> (Int, Int, [UInt8]) {
  let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: p) as CFURL, nil)!
  let img = CGImageSourceCreateImageAtIndex(src, 0, nil)!
  let w = img.width, h = img.height
  var buf = [UInt8](repeating: 0, count: w * h * 4)
  let ctx = CGContext(data: &buf, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.draw(img, in: CGRect(x: 0, y: 0, width: w, height: h))
  return (w, h, buf)
}
let (w1, h1, a) = load(CommandLine.arguments[1]); let (w2, h2, b) = load(CommandLine.arguments[2])
guard w1 == w2 && h1 == h2 else { print("size", w1, h1, w2, h2); exit(0) }
var n = 0; var minx = w1, miny = h1, maxx = 0, maxy = 0
for y in 0..<h1 { for x in 0..<min(w1, 2370) { let i = (y * w1 + x) * 4
  let d = max(abs(Int(a[i]) - Int(b[i])), abs(Int(a[i+1]) - Int(b[i+1])), abs(Int(a[i+2]) - Int(b[i+2])))
  if d > (Int(ProcessInfo.processInfo.environment["T"] ?? "40") ?? 40) { n += 1; minx = min(minx, x); maxx = max(maxx, x); miny = min(miny, y); maxy = max(maxy, y) } } }
print(n == 0 ? "same" : "DIFF \(n) px box \(minx),\(miny)-\(maxx),\(maxy) (px; /2 for pt)")
