import AppIntents
import SwiftUI
import WidgetKit

struct NewSessionControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "sh.zeron.control.new-session") {
            ControlWidgetButton(action: NewSessionIntent()) {
                Label("New Session", systemImage: "plus.bubble")
            }
        }
        .displayName("New Session")
        .description("Start a new Zeron session.")
    }
}

struct TalkToCodexControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "sh.zeron.control.talk") {
            ControlWidgetButton(action: TalkToCodexIntent()) {
                Label("Talk to Codex", systemImage: "waveform")
            }
        }
        .displayName("Talk to Codex")
        .description("Start a voice call with Codex on your default host.")
    }
}
