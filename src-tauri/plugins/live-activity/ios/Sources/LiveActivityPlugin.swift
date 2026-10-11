import ActivityKit
import AVFoundation
import Foundation
import SwiftRs
import Tauri
import WidgetKit

/// Mirrors the Rust side's `ActivityContent` (src-tauri/src/live_activity.rs).
struct UpdateArgs: Decodable {
    let title: String
    let subtitle: String?
    let appName: String?
    let isFiltering: Bool
    let nextCueAction: String?
    let nextCueCategory: String?
    let nextCueTime: String?
    /// Unix epoch milliseconds; see `FamilyFilterStatus.nextCueAt`.
    let nextCueEpochMs: Double?

    var status: FamilyFilterStatus {
        FamilyFilterStatus(
            title: title,
            subtitle: subtitle,
            appName: appName,
            isFiltering: isFiltering,
            nextCueAction: nextCueAction,
            nextCueCategory: nextCueCategory,
            nextCueTime: nextCueTime,
            nextCueAt: nextCueEpochMs.map { Date(timeIntervalSince1970: $0 / 1000) }
        )
    }
}

class LiveActivityPlugin: Plugin {
    private let keepAlive = BackgroundKeepAlive()
    /// Serializes ActivityKit work so a slow update can't be overtaken by
    /// the next one, and two requests can't race into two activities.
    private var queue: Task<Void, Never>?

    /// The app re-sends unchanged content at least this often (see
    /// `RESEND_INTERVAL` in live_activity.rs). If those resends stop because
    /// the app was killed or suspended, the activity goes stale and says so
    /// rather than silently showing outdated cue info.
    private static let staleAfter: TimeInterval = 180

    @objc public func update(_ invoke: Invoke) throws {
        let status = try invoke.parseArgs(UpdateArgs.self).status
        keepAlive.start()
        publishToWidgets(status)
        enqueue {
            let content = ActivityContent(
                state: status,
                staleDate: Date().addingTimeInterval(Self.staleAfter)
            )
            if let activity = Activity<FamilyFilterActivityAttributes>.activities.first {
                await activity.update(content)
            } else if ActivityAuthorizationInfo().areActivitiesEnabled {
                do {
                    _ = try Activity.request(
                        attributes: FamilyFilterActivityAttributes(),
                        content: content
                    )
                } catch {
                    // Expected when the app isn't in the foreground (an
                    // ActivityKit rule); the next update retries.
                    NSLog("[live-activity] request failed: \(error)")
                }
            }
        }
        invoke.resolve()
    }

    @objc public func end(_ invoke: Invoke) {
        keepAlive.stop()
        publishToWidgets(.idle)
        enqueue {
            for activity in Activity<FamilyFilterActivityAttributes>.activities {
                await activity.end(nil, dismissalPolicy: .immediate)
            }
        }
        invoke.resolve()
    }

    private func publishToWidgets(_ status: FamilyFilterStatus) {
        FamilyFilterStatusStore.save(status)
        WidgetCenter.shared.reloadAllTimelines()
    }

    private func enqueue(_ work: @escaping @MainActor () async -> Void) {
        let previous = queue
        queue = Task { @MainActor in
            await previous?.value
            await work()
        }
    }
}

/// Keeps the app running while the phone is locked so the Rust heartbeat
/// (src-tauri/src/control.rs `spawn_heartbeat`) keeps applying cues and its
/// Apple TV sockets stay open. Uses the `audio` background mode
/// (Info.ios.plist) by looping a silent buffer, mixed with other audio so it
/// never interrupts or ducks the user's own sound.
final class BackgroundKeepAlive {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let buffer: AVAudioPCMBuffer
    /// Whether the keep-alive should be running; distinct from whether it
    /// currently is, so it can be restored after an interruption (e.g. a
    /// phone call) ends.
    private var wanted = false
    private var observers: [NSObjectProtocol] = []

    init() {
        let format = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1)!
        buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 44_100)!
        buffer.frameLength = buffer.frameCapacity
        // Buffer memory isn't guaranteed to be zeroed.
        if let channel = buffer.floatChannelData?[0] {
            channel.update(repeating: 0, count: Int(buffer.frameLength))
        }
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: format)

        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification, object: nil, queue: .main
        ) { [weak self] note in
            guard let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  AVAudioSession.InterruptionType(rawValue: raw) == .ended else { return }
            self?.restartIfWanted()
        })
        observers.append(center.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main
        ) { [weak self] _ in
            self?.restartIfWanted()
        })
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    func start() {
        wanted = true
        guard !engine.isRunning else { return }
        do {
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playback, options: [.mixWithOthers])
            try session.setActive(true)
            try engine.start()
            player.scheduleBuffer(buffer, at: nil, options: .loops)
            player.play()
        } catch {
            NSLog("[live-activity] keep-alive failed to start: \(error)")
        }
    }

    func stop() {
        wanted = false
        guard engine.isRunning else { return }
        player.stop()
        engine.stop()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    private func restartIfWanted() {
        guard wanted else { return }
        player.stop()
        engine.stop()
        start()
    }
}

@_cdecl("init_plugin_live_activity")
func initPlugin() -> Plugin {
    return LiveActivityPlugin()
}
