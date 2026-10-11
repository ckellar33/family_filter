import ActivityKit
import SwiftUI
import WidgetKit

/// The Lock Screen / Dynamic Island Live Activity. Started, updated and
/// ended by the app (plugins/live-activity) while auto-filter is on and an
/// Apple TV is connected.
struct FamilyFilterLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: FamilyFilterActivityAttributes.self) { context in
            LockScreenActivityView(status: context.state, isStale: context.isStale)
                .padding(14)
                .activityBackgroundTint(Color.black.opacity(0.6))
                .activitySystemActionForegroundColor(.white)
        } dynamicIsland: { context in
            let status = context.state
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Image(systemName: "shield.lefthalf.filled")
                        .foregroundStyle(status.isFiltering ? .green : .orange)
                        .font(.title2)
                }
                DynamicIslandExpandedRegion(.center) {
                    Text(status.title).font(.headline).lineLimit(1)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    NextCueRow(status: status)
                }
            } compactLeading: {
                Image(systemName: "shield.lefthalf.filled")
                    .foregroundStyle(status.isFiltering ? .green : .orange)
            } compactTrailing: {
                if let action = status.nextCueAction {
                    if let at = status.nextCueAt, at > Date() {
                        Text(timerInterval: Date()...at, countsDown: true)
                            .monospacedDigit()
                            .frame(maxWidth: 44)
                    } else {
                        Text(action).font(.caption2.bold())
                    }
                }
            } minimal: {
                Image(systemName: "shield.lefthalf.filled")
                    .foregroundStyle(status.isFiltering ? .green : .orange)
            }
        }
    }
}

struct LockScreenActivityView: View {
    let status: FamilyFilterStatus
    let isStale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            NowPlayingHeader(status: status)
            Divider()
            if isStale {
                // The app stopped sending its periodic refresh (see
                // `staleAfter` in LiveActivityPlugin.swift), so it's no
                // longer running and nothing is being filtered.
                Label("Family Filter stopped. Open the app to resume filtering.",
                      systemImage: "exclamationmark.triangle.fill")
                    .font(.subheadline)
                    .foregroundStyle(.orange)
            } else {
                NextCueRow(status: status)
            }
        }
        .foregroundStyle(.white)
    }
}

#Preview("Lock Screen", as: .content, using: FamilyFilterActivityAttributes()) {
    FamilyFilterLiveActivity()
} contentStates: {
    FamilyFilterStatus.preview
}
