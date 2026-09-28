#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <dispatch/dispatch.h>

// 0 = authorized, 1 = denied, 2 = restricted. The supervisor can terminate
// this private child while the user is deciding in the system permission UI.
int maac_microphone_authorize(void) {
    @autoreleasepool {
        AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
        if (status == AVAuthorizationStatusAuthorized) return 0;
        if (status == AVAuthorizationStatusRestricted) return 2;
        if (status == AVAuthorizationStatusDenied) return 1;
        dispatch_semaphore_t finished = dispatch_semaphore_create(0);
        __block BOOL granted = NO;
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL allowed) {
            granted = allowed;
            dispatch_semaphore_signal(finished);
        }];
        dispatch_semaphore_wait(finished, DISPATCH_TIME_FOREVER);
        return granted ? 0 : 1;
    }
}
