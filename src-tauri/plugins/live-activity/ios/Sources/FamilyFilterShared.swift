// Compiled into BOTH the app (as part of this plugin) and the widget
// extension (src-tauri/ios/project.yml lists this file as one of the
// extension's sources). ActivityKit matches a running activity to the
// extension's `ActivityConfiguration` by the attributes type, and the widgets
// decode what the app saved, so both sides must share these exact
// definitions. Keep this file free of Tauri/UIKit imports.

import ActivityKit
import Foundation

/// Everything the Live Activity and widgets display. Also the Live
/// Activity's `ContentState`.
public struct FamilyFilterStatus: Codable, Hashable {
    /// What's playing, or "Nothing playing".
    public var title: String
    /// Series name (for an episode) or the app's own secondary line.
    public var subtitle: String?
    /// Friendly streaming-app name, e.g. "Disney+".
    public var appName: String?
    /// Whether auto-filter is on AND the playing title matched the loaded
    /// filter list, i.e. cues are actually being applied right now.
    public var isFiltering: Bool
    /// "MUTE" or "SKIP".
    public var nextCueAction: String?
    public var nextCueCategory: String?
    /// The cue's position in the media, e.g. "12:34".
    public var nextCueTime: String?
    /// Wall-clock moment the next cue starts. Only set while playback is
    /// advancing, so views can show a live countdown that keeps ticking
    /// without any further updates from the app.
    public var nextCueAt: Date?
    /// False once auto-filter has been turned off or the Apple TV
    /// disconnected; the widgets then show an idle state.
    public var isActive: Bool

    public init(
        title: String,
        subtitle: String? = nil,
        appName: String? = nil,
        isFiltering: Bool = false,
        nextCueAction: String? = nil,
        nextCueCategory: String? = nil,
        nextCueTime: String? = nil,
        nextCueAt: Date? = nil,
        isActive: Bool = true
    ) {
        self.title = title
        self.subtitle = subtitle
        self.appName = appName
        self.isFiltering = isFiltering
        self.nextCueAction = nextCueAction
        self.nextCueCategory = nextCueCategory
        self.nextCueTime = nextCueTime
        self.nextCueAt = nextCueAt
        self.isActive = isActive
    }

    public static let idle = FamilyFilterStatus(title: "Family Filter", isActive: false)

    public static let preview = FamilyFilterStatus(
        title: "Chapter 1",
        subtitle: "The Mandalorian",
        appName: "Disney+",
        isFiltering: true,
        nextCueAction: "SKIP",
        nextCueCategory: "Violence",
        nextCueTime: "12:34",
        nextCueAt: Date().addingTimeInterval(95)
    )
}

public struct FamilyFilterActivityAttributes: ActivityAttributes {
    public typealias ContentState = FamilyFilterStatus

    public init() {}
}

// MARK: - App Group storage (app writes, widgets read)

public enum FamilyFilterStatusStore {
    public static let appGroup = "group.com.ckellar33.familyfilter"
    private static let key = "familyFilterStatus"

    public static func save(_ status: FamilyFilterStatus) {
        guard let data = try? JSONEncoder().encode(status) else { return }
        UserDefaults(suiteName: appGroup)?.set(data, forKey: key)
    }

    public static func load() -> FamilyFilterStatus? {
        guard let data = UserDefaults(suiteName: appGroup)?.data(forKey: key) else {
            return nil
        }
        return try? JSONDecoder().decode(FamilyFilterStatus.self, from: data)
    }
}
