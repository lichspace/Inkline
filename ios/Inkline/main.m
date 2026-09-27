#import <UIKit/UIKit.h>

extern void inkline_start(void);

double inkline_safe_area_top(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (![scene isKindOfClass:[UIWindowScene class]]) {
            continue;
        }
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            if ([NSStringFromClass(window.class) isEqualToString:@"WinitUIWindow"]) {
                return window.safeAreaInsets.top;
            }
        }
    }
    return 0.0;
}

@interface InklineSceneDelegate : UIResponder <UIWindowSceneDelegate>

@property(nonatomic, strong) UIWindow *window;
@property(nonatomic, weak) UIWindowScene *windowScene;

@end

@implementation InklineSceneDelegate

- (void)attachWinitWindow:(UIWindow *)candidate {
    if (![NSStringFromClass(candidate.class) isEqualToString:@"WinitUIWindow"] ||
        self.window == candidate || self.windowScene == nil) {
        return;
    }

    candidate.windowScene = self.windowScene;
    self.window = candidate;
    [candidate makeKeyAndVisible];
}

- (void)windowDidBecomeVisible:(NSNotification *)notification {
    if ([notification.object isKindOfClass:[UIWindow class]]) {
        [self attachWinitWindow:(UIWindow *)notification.object];
    }
}

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)connectionOptions {
    if (![scene isKindOfClass:[UIWindowScene class]]) {
        return;
    }

    // winit creates its UIWindow while UIApplication finishes launching. On
    // scene-based iOS releases, attach that existing Metal-backed window to the
    // UIWindowScene before making it visible.
    self.windowScene = (UIWindowScene *)scene;
    [NSNotificationCenter.defaultCenter addObserver:self
                                           selector:@selector(windowDidBecomeVisible:)
                                               name:UIWindowDidBecomeVisibleNotification
                                             object:nil];

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    for (UIWindow *candidate in UIApplication.sharedApplication.windows) {
        [self attachWinitWindow:candidate];
    }
#pragma clang diagnostic pop
}

- (void)dealloc {
    [NSNotificationCenter.defaultCenter removeObserver:self];
}

@end

int main(void) {
    @autoreleasepool {
        inkline_start();
    }
    return 0;
}
