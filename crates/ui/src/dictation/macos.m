// Main-thread-owned Speech/AVAudioEngine bridge. No audio files, network
// fallback, engine callbacks, or Rust pointers are retained by native blocks.
#import <AppKit/AppKit.h>
#import <AVFoundation/AVFoundation.h>
#import <Speech/Speech.h>

@interface ZeronDictation : NSObject
@property(nonatomic) BOOL active;
@property(nonatomic) BOOL finishing;
@property(nonatomic) BOOL tapped;
@property(nonatomic) BOOL announcedEnd;
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) SFSpeechRecognizer *recognizer;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *request;
@property(nonatomic, strong) SFSpeechRecognitionTask *task;
@property(nonatomic, strong) NSMutableArray<NSDictionary *> *events;
@property(nonatomic, strong) NSString *polled;
@property(nonatomic, strong) id activationObserver;
@property(nonatomic, strong) id windowObserver;
@property(nonatomic, weak) NSWindow *window;
@property(nonatomic, strong) id configurationObserver;
- (void)start;
- (void)listen;
- (void)finish;
- (void)cancel;
@end

@implementation ZeronDictation
- (instancetype)init {
    if ((self = [super init])) {
        _active = YES;
        _events = [NSMutableArray new];
    }
    return self;
}
- (void)announce:(NSString *)text {
    if (NSApp) {
        NSAccessibilityPostNotificationWithUserInfo(NSApp,
            NSAccessibilityAnnouncementRequestedNotification,
            @{NSAccessibilityAnnouncementKey: text,
              NSAccessibilityPriorityKey: @(NSAccessibilityPriorityMedium)});
    }
}
- (void)emit:(NSString *)kind text:(NSString *)text {
    if (self.active) {
        if ([kind isEqualToString:@"listening"]) {
            [self announce:@"Listening on this Mac."];
        } else if (![kind isEqualToString:@"partial"]) {
            self.announcedEnd = YES;
            [self announce:([kind isEqualToString:@"final"] || [kind isEqualToString:@"cancelled"])
                ? @"Dictation stopped." : text];
        }
        // Coalesce partials if the UI is busy; memory never grows with audio.
        if ([kind isEqualToString:@"partial"] &&
            [self.events.lastObject[@"kind"] isEqualToString:@"partial"]) {
            [self.events removeLastObject];
        }
        [self.events addObject:text ? @{@"kind": kind, @"text": text} : @{@"kind": kind}];
    }
}
- (void)stopAudio {
    [self.engine stop];
    if (self.tapped) {
        [self.engine.inputNode removeTapOnBus:0];
        self.tapped = NO;
    }
    [self.request endAudio];
    self.engine = nil;
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    if (self.activationObserver) [center removeObserver:self.activationObserver];
    if (self.configurationObserver) [center removeObserver:self.configurationObserver];
    if (self.windowObserver) [center removeObserver:self.windowObserver];
    self.activationObserver = nil;
    self.windowObserver = nil;
    self.configurationObserver = nil;
}
- (void)cancel {
    if (self.active && !self.announcedEnd) [self announce:@"Dictation stopped."];
    self.active = NO;
    [self stopAudio];
    [self.task cancel];
    self.task = nil;
    self.request = nil;
    self.recognizer = nil;
}
- (void)fail:(NSString *)kind text:(NSString *)message {
    [self emit:kind text:message];
    [self cancel];
}
- (void)start {
    // A bare cargo executable attributes TCC prompts to the terminal and may
    // terminate for missing usage descriptions. Only bundles are supported.
    self.window = NSApp.keyWindow;
    NSBundle *bundle = NSBundle.mainBundle;
    if (![bundle.bundlePath.pathExtension isEqualToString:@"app"] ||
        ![bundle objectForInfoDictionaryKey:@"NSMicrophoneUsageDescription"] ||
        ![bundle objectForInfoDictionaryKey:@"NSSpeechRecognitionUsageDescription"]) {
        [self fail:@"unavailable" text:@"Dictation requires the Zeron app bundle. Use scripts/run-macos-dev.sh for development."];
        return;
    }
    self.recognizer = [[SFSpeechRecognizer alloc] initWithLocale:NSLocale.currentLocale];
    if (!self.recognizer.supportsOnDeviceRecognition || !self.recognizer.available) {
        [self fail:@"unavailable" text:@"On-device dictation is unavailable for this Mac’s current language. No audio was sent to a server."];
        return;
    }
    [self announce:@"Requesting dictation permission."];
    __weak ZeronDictation *weakSelf = self;
    [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
        dispatch_async(dispatch_get_main_queue(), ^{
            ZeronDictation *session = weakSelf;
            if (!session.active) return;
            if (status != SFSpeechRecognizerAuthorizationStatusAuthorized) {
                [session fail:@"denied" text:@"Allow Speech Recognition for Zeron in System Settings → Privacy & Security, then retry dictation."];
                return;
            }
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    ZeronDictation *session = weakSelf;
                    if (!session.active) return;
                    if (!granted) {
                        [session fail:@"denied" text:@"Allow Microphone access for Zeron in System Settings → Privacy & Security, then retry dictation."];
                        return;
                    }
                    [session listen];
                });
            }];
        });
    }];
}
- (void)listen {
    if (!self.active || self.finishing) return;
    if (!NSApp.active || !self.window || NSApp.keyWindow != self.window) {
        [self fail:@"cancelled" text:nil];
        return;
    }
    if (!self.recognizer.available || !self.recognizer.supportsOnDeviceRecognition) {
        [self fail:@"unavailable" text:@"On-device recognition is unavailable. Your draft is safe; try again later."];
        return;
    }
    self.request = [SFSpeechAudioBufferRecognitionRequest new];
    self.request.requiresOnDeviceRecognition = YES;
    self.request.shouldReportPartialResults = YES;
    self.request.taskHint = SFSpeechRecognitionTaskHintDictation;
    self.engine = [AVAudioEngine new];
    __weak ZeronDictation *weakSelf = self;
    // Device removal/reconfiguration and application deactivation release the
    // microphone immediately, independently of the GPUI polling interval.
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    self.activationObserver = [center addObserverForName:NSApplicationDidResignActiveNotification
        object:NSApp queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
            [weakSelf fail:@"cancelled" text:nil];
        }];
    self.windowObserver = [center addObserverForName:NSWindowDidResignKeyNotification
        object:self.window queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
            [weakSelf fail:@"cancelled" text:nil];
        }];
    self.configurationObserver = [center addObserverForName:AVAudioEngineConfigurationChangeNotification
        object:self.engine queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
            [weakSelf fail:@"failed" text:@"The microphone changed or disconnected. Your draft is safe; retry dictation."];
        }];
    @try {
        AVAudioInputNode *input = self.engine.inputNode;
        AVAudioFormat *format = [input outputFormatForBus:0];
        if (format.sampleRate <= 0 || format.channelCount == 0) {
            [self fail:@"unavailable" text:@"No microphone is available. Connect one and retry dictation."];
            return;
        }
        // The audio thread captures only the request, never UI/session state.
        SFSpeechAudioBufferRecognitionRequest *request = self.request;
        [input installTapOnBus:0 bufferSize:1024 format:format block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
            [request appendAudioPCMBuffer:buffer];
        }];
        self.tapped = YES;
        self.task = [self.recognizer recognitionTaskWithRequest:self.request
            resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    ZeronDictation *session = weakSelf;
                    if (!session.active) return;
                    if (error) {
                        // Preserve useful text, but a failed finalization must
                        // never activate a pending Send.
                        if (result) [session emit:@"partial" text:result.bestTranscription.formattedString];
                        [session fail:@"failed" text:@"Dictation stopped unexpectedly. Your draft is safe; retry dictation."];
                        return;
                    }
                    if (result) {
                        [session emit:result.final ? @"final" : @"partial"
                            text:result.bestTranscription.formattedString];
                        if (result.final) [session cancel];
                    }
                });
            }];
        if (!self.task) {
            [self fail:@"failed" text:@"Could not start on-device recognition. Your draft is safe; retry dictation."];
            return;
        }
        [self.engine prepare];
        NSError *error = nil;
        if (![self.engine startAndReturnError:&error]) {
            [self fail:@"failed" text:@"Could not start the microphone. Your draft is safe; retry dictation."];
            return;
        }
        [self emit:@"listening" text:nil];
    } @catch (NSException *exception) {
        [self fail:@"failed" text:@"Could not configure the microphone. Your draft is safe; retry dictation."];
    }
}
- (void)finish {
    if (!self.active || self.finishing) return;
    self.finishing = YES;
    if (!self.task) {
        [self emit:@"final" text:@""];
        [self cancel]; // Also invalidates an outstanding permission callback.
        return;
    }
    [self announce:@"Finishing dictation."];
    [self stopAudio]; // endAudio allows the recognizer to deliver its final result.
}
@end

void *zeron_dictation_start(void) {
    NSCAssert(NSThread.isMainThread, @"Dictation must run on the UI thread");
    ZeronDictation *session = [ZeronDictation new];
    [session start];
    return (__bridge_retained void *)session;
}
const char *zeron_dictation_poll(void *pointer) {
    ZeronDictation *session = (__bridge ZeronDictation *)pointer;
    if (!session.events.count) return NULL;
    NSDictionary *event = session.events.firstObject;
    [session.events removeObjectAtIndex:0];
    NSData *json = [NSJSONSerialization dataWithJSONObject:event options:0 error:nil];
    session.polled = [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding];
    return session.polled.UTF8String;
}
void zeron_dictation_finish(void *pointer) {
    [(__bridge ZeronDictation *)pointer finish];
}
void zeron_dictation_release(void *pointer) {
    ZeronDictation *session = (__bridge_transfer ZeronDictation *)pointer;
    [session cancel];
}
