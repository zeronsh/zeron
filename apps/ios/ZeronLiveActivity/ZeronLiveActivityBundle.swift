import SwiftUI
import WidgetKit

@main
struct ZeronLiveActivityBundle: WidgetBundle {
    var body: some Widget {
        SessionsWidget()
        UsageWidget()
        DevicesWidget()
        ActionsWidget()
        NewSessionControl()
        TalkToCodexControl()
        VoiceLiveActivity()
    }
}
