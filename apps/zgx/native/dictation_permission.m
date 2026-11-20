#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
// No Rust callbacks or audio retained by permission blocks.
int zeron_microphone_permission(void) {
    NSBundle *bundle = NSBundle.mainBundle;
    if (![bundle.bundlePath.pathExtension isEqualToString:@"app"] || ![bundle objectForInfoDictionaryKey:@"NSMicrophoneUsageDescription"]) return -2;
    AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (status == AVAuthorizationStatusAuthorized) return 1;
    if (status == AVAuthorizationStatusNotDetermined) return 0;
    return -1;
}
void zeron_request_microphone(void) {
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {}];
}
uintptr_t zeron_microphone_window(void) {
    return NSApp.active ? (uintptr_t)(__bridge void *)NSApp.keyWindow : 0;
}
