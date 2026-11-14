#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
// No audio is retained by permission blocks; zeron_request_microphone_then's
// block retains only its Rust callback and context, until it answers once.
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
// The answer, to `done` with `context` (once, on whatever queue AVFoundation
// picks: the caller hands it to its own thread).
void zeron_request_microphone_then(void (*done)(void *context, int granted), void *context) {
    [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
        done(context, granted ? 1 : 0);
    }];
}
uintptr_t zeron_microphone_window(void) {
    return NSApp.active ? (uintptr_t)(__bridge void *)NSApp.keyWindow : 0;
}
