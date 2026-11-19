#include <stdbool.h>
#import <Foundation/Foundation.h>
#import <LocalAuthentication/LocalAuthentication.h>
#import <TargetConditionals.h>
#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#else
#import <UIKit/UIKit.h>
#endif

extern void gipny_screen_hidden(void);
extern void gipny_device_auth_finished(void *context, bool success);

void gipny_observe_screen(void) {
#if TARGET_OS_OSX
    NSNotificationCenter *center = NSWorkspace.sharedWorkspace.notificationCenter;
    NSArray *events = @[NSWorkspaceScreensDidSleepNotification, NSWorkspaceWillSleepNotification,
                        NSWorkspaceSessionDidResignActiveNotification];
#else
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    NSArray *events = @[UIApplicationProtectedDataWillBecomeUnavailable];
#endif
    for (NSString *name in events) {
        [center addObserverForName:name object:nil queue:nil usingBlock:^(NSNotification *note) {
            gipny_screen_hidden();
        }];
    }
}

bool gipny_device_auth_available(void) {
    return [[LAContext new] canEvaluatePolicy:LAPolicyDeviceOwnerAuthentication error:nil];
}

void gipny_device_authenticate(const char *reason, void *context) {
    NSString *prompt = [NSString stringWithUTF8String:reason];
    LAContext *auth = [LAContext new];
    [auth evaluatePolicy:LAPolicyDeviceOwnerAuthentication localizedReason:prompt reply:^(BOOL success, NSError *error) {
        [auth invalidate];
        gipny_device_auth_finished(context, success);
    }];
}
