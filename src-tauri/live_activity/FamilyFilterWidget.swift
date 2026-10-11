import SwiftUI
import WidgetKit

/// Home Screen (small/medium) and Lock Screen (accessory) widget.
struct FamilyFilterWidget: Widget {
    let kind: String = "FamilyFilterWidget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: Provider()) { entry in
            FamilyFilterWidgetView(status: entry.status)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        .configurationDisplayName("Now Playing")
        .description("What's playing on your Apple TV and the next filter cue.")
        .supportedFamilies([
            .systemSmall, .systemMedium,
            .accessoryRectangular, .accessoryInline, .accessoryCircular,
        ])
    }
}

struct FamilyFilterWidgetView: View {
    @Environment(\.widgetFamily) private var family
    let status: FamilyFilterStatus

    var body: some View {
        switch family {
        case .accessoryInline:
            inline
        case .accessoryCircular:
            circular
        case .accessoryRectangular:
            rectangular
        default:
            system
        }
    }

    // MARK: Lock Screen

    @ViewBuilder private var inline: some View {
        if !status.isActive {
            Label("Filter off", systemImage: "shield.slash")
        } else if let action = status.nextCueAction, let time = status.nextCueTime {
            Label("\(action) at \(time)", systemImage: "shield.lefthalf.filled")
        } else {
            Label(status.title, systemImage: "shield.lefthalf.filled")
        }
    }

    private var circular: some View {
        ZStack {
            AccessoryWidgetBackground()
            Image(systemName: status.isActive ? "shield.lefthalf.filled" : "shield.slash")
                .font(.title2)
        }
        .widgetAccentable()
    }

    private var rectangular: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(status.isActive ? status.title : "Family Filter")
                .font(.headline)
                .widgetAccentable()
                .lineLimit(1)
            if !status.isActive {
                Text("Auto-filter is off").font(.caption)
            } else if status.nextCueAction != nil {
                NextCueText(status: status).font(.caption)
            } else {
                Text(status.isFiltering ? "No more cues" : "No filter for this title")
                    .font(.caption)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: Home Screen

    private var system: some View {
        VStack(alignment: .leading, spacing: 6) {
            NowPlayingHeader(status: status)
            Spacer(minLength: 0)
            if status.isActive {
                NextCueRow(status: status)
            } else {
                Text("Auto-filter is off")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

// MARK: - Shared pieces (also used by the Live Activity)

struct NowPlayingHeader: View {
    let status: FamilyFilterStatus

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if let subtitle = status.subtitle, !subtitle.isEmpty {
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Text(status.title)
                .font(.headline)
                .lineLimit(2)
            HStack(spacing: 4) {
                FilterStatusDot(status: status)
                if let app = status.appName, !app.isEmpty {
                    Text("· \(app)")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
    }
}

struct FilterStatusDot: View {
    let status: FamilyFilterStatus

    private var label: String {
        if !status.isActive { return "Filter off" }
        return status.isFiltering ? "Filtering" : "No filter for title"
    }

    private var color: Color {
        status.isActive && status.isFiltering ? .green : .orange
    }

    var body: some View {
        HStack(spacing: 4) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(label)
                .font(.caption2)
                .foregroundStyle(color)
                .lineLimit(1)
        }
    }
}

struct NextCueRow: View {
    let status: FamilyFilterStatus

    var body: some View {
        if let action = status.nextCueAction {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                CueBadge(action: action)
                NextCueText(status: status)
                    .font(.subheadline.weight(.semibold))
                if let category = status.nextCueCategory, !category.isEmpty {
                    Text(category)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        } else {
            Text(status.isFiltering ? "No more cues" : "No filter for this title")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
    }
}

struct CueBadge: View {
    let action: String

    var body: some View {
        Text(action)
            .font(.system(size: 11, weight: .bold, design: .monospaced))
            .foregroundStyle(.white)
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(action == "SKIP" ? Color.orange : Color.red, in: .rect(cornerRadius: 4))
    }
}

/// "in 1:35" as a live countdown while playback is advancing (rendered by
/// the system, so it keeps ticking with no app updates), otherwise the cue's
/// position in the media, e.g. "at 12:34".
struct NextCueText: View {
    let status: FamilyFilterStatus

    var body: some View {
        if let at = status.nextCueAt, at > Date() {
            HStack(spacing: 3) {
                Text("in")
                Text(timerInterval: Date()...at, countsDown: true)
                    .monospacedDigit()
            }
        } else if status.nextCueAt != nil {
            Text("now")
        } else if let time = status.nextCueTime {
            Text("at \(time)")
        }
    }
}

#Preview(as: .systemMedium) {
    FamilyFilterWidget()
} timeline: {
    StatusEntry(date: .now, status: .preview)
    StatusEntry(date: .now, status: .idle)
}

#Preview(as: .accessoryRectangular) {
    FamilyFilterWidget()
} timeline: {
    StatusEntry(date: .now, status: .preview)
}
