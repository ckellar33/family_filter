import WidgetKit
import Foundation

/// Reads whatever the app last published to the App Group (see
/// `FamilyFilterStatusStore`). The app calls `reloadAllTimelines()` whenever
/// that changes, so a single-entry timeline is enough.
struct Provider: TimelineProvider {
    func placeholder(in context: Context) -> StatusEntry {
        StatusEntry(date: Date(), status: .preview)
    }

    func getSnapshot(in context: Context, completion: @escaping (StatusEntry) -> Void) {
        let status = context.isPreview ? .preview : (FamilyFilterStatusStore.load() ?? .idle)
        completion(StatusEntry(date: Date(), status: status))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<StatusEntry>) -> Void) {
        let entry = StatusEntry(date: Date(), status: FamilyFilterStatusStore.load() ?? .idle)
        completion(Timeline(entries: [entry], policy: .never))
    }
}

struct StatusEntry: TimelineEntry {
    let date: Date
    let status: FamilyFilterStatus
}
