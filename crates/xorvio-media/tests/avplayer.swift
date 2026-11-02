// Plays live buffers with AVFoundation, the engine behind WebKit's own HLS
// player on macOS, and prints what each does as JSON lines: played at once,
// paused at 10 s for 6 s, then sent back to the start of the window at 22 s.
// Run by the timeshift tests: swift avplayer.swift <name>=<url>...

import AVFoundation
import Foundation

struct Watched {
    let name: String
    let item: AVPlayerItem
    let player: AVPlayer
}

let watched: [Watched] = CommandLine.arguments.dropFirst().map { argument in
    let parts = argument.split(separator: "=", maxSplits: 1).map(String.init)
    let item = AVPlayerItem(url: URL(string: parts[1])!)
    let player = AVPlayer(playerItem: item)
    player.isMuted = true
    return Watched(name: parts[0], item: item, player: player)
}

func fourCC(_ code: FourCharCode) -> String {
    let bytes = [24, 16, 8, 0].map { UInt8((code >> $0) & 0xFF) }
    return String(bytes: bytes, encoding: .ascii) ?? "?"
}

let started = Date()

func report(_ event: String) {
    for w in watched {
        var tracks: [String] = []
        for track in w.item.tracks {
            guard let asset = track.assetTrack else { continue }
            for description in asset.formatDescriptions as! [CMFormatDescription] {
                var text = "\(asset.mediaType.rawValue):\(fourCC(CMFormatDescriptionGetMediaSubType(description)))"
                if let basic = CMAudioFormatDescriptionGetStreamBasicDescription(description) {
                    text += ":\(basic.pointee.mChannelsPerFrame)ch"
                }
                tracks.append(text)
            }
        }
        let seekable = w.item.seekableTimeRanges.map { $0.timeRangeValue }.map { [$0.start.seconds, $0.end.seconds] }
        let errors = w.item.errorLog()?.events.map { "\($0.errorStatusCode) \($0.errorComment ?? "")" } ?? []
        let line: [String: Any] = [
            "name": w.name,
            "event": event,
            "t": Date().timeIntervalSince(started),
            "time": w.item.currentTime().seconds.isFinite ? w.item.currentTime().seconds : -1,
            "status": w.item.status.rawValue,
            "rate": w.player.rate,
            "width": w.item.presentationSize.width,
            "seekable": seekable,
            "tracks": tracks,
            "error": w.item.error.map { "\($0)" } ?? NSNull(),
            "errorLog": errors,
        ]
        let data = try! JSONSerialization.data(withJSONObject: line)
        print(String(data: data, encoding: .utf8)!)
        fflush(stdout)
    }
}

for w in watched { w.player.play() }
var tick = 0
Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
    tick += 1
    switch tick {
    case 10:
        watched.forEach { $0.player.pause() }
        report("paused")
    case 16:
        watched.forEach { $0.player.play() }
        report("resumed")
    case 22:
        for w in watched {
            if let first = w.item.seekableTimeRanges.first?.timeRangeValue {
                w.player.seek(to: first.start + CMTime(seconds: 1, preferredTimescale: 600))
            }
        }
        report("back")
    default:
        report("tick")
    }
    if tick >= 30 { exit(0) }
}
RunLoop.main.run()
