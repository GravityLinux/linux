// SPDX-License-Identifier: GPL-2.0-only
// Own pairing/L2CAP test using the public macOS IOBluetooth SDK API.
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <IOBluetooth/IOBluetooth.h>

@interface M5BTTest : NSObject <IOBluetoothDevicePairDelegate, IOBluetoothL2CAPChannelDelegate>
@property(strong) IOBluetoothDevice *device;
@property(strong) IOBluetoothDevicePair *pair;
@property(strong) IOBluetoothL2CAPChannel *channel;
@property(strong) NSData *challenge;
@property BOOL done;
@property BOOL dataStarted;
@property BOOL payloadStarted;
@property(strong) IOBluetoothUserNotification *notification;
@property NSUInteger iterations;
@property NSUInteger received;
@property int result;
@property(strong) NSDate *deadline;
@end
@implementation M5BTTest
- (void)tick:(NSTimer *)timer {
    if(!self.done && [self.deadline timeIntervalSinceNow]>0) return;
    [timer invalidate];
    [NSApp stop:nil];
    [NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0] atStart:NO];
}
- (void)sendChallenge {
    NSMutableData *next=[self.challenge mutableCopy];
    uint64_t sequence=CFSwapInt64HostToLittle(self.received);
    memcpy((uint8_t *)next.mutableBytes+next.length-8,&sequence,8);
    for(NSUInteger i=80;i+8<next.length;i++) ((uint8_t *)next.mutableBytes)[i]=(uint8_t)(i+self.received);
    self.challenge=next;
    IOReturn error=[self.channel writeSync:(void *)self.challenge.bytes length:(UInt16)self.challenge.length];
    if(error || self.received % 128==0) { printf("MAC_MINI_L2CAP_WRITE status=%d completed=%lu bytes=%lu\n",error,(unsigned long)self.received,(unsigned long)self.challenge.length); fflush(stdout); }
    if(error) { self.result=5; self.done=YES; }
}
- (void)opened:(IOBluetoothUserNotification *)note channel:(IOBluetoothL2CAPChannel *)channel {
    (void)note;
    printf("MAC_MINI_CHANNEL_NOTIFICATION\n"); fflush(stdout);
    [channel setDelegate:self];
    if(!self.payloadStarted) { self.payloadStarted=YES; [self performSelector:@selector(sendChallenge) withObject:nil afterDelay:0.1]; }
}
- (void)startData {
    if(self.dataStarted) return;
    self.dataStarted=YES;
    if (![self.device isPaired]) { self.result=3; self.done=YES; return; }
    printf("MAC_MINI_BASEBAND connected=%d paired=%d\n",[self.device isConnected],[self.device isPaired]); fflush(stdout);
    if(![self.device isConnected]) {
        IOReturn baseband=[self.device openConnection];
        printf("MAC_MINI_BASEBAND_OPEN status=%d connected=%d\n",baseband,[self.device isConnected]); fflush(stdout);
        if(baseband) { self.result=8; self.done=YES; return; }
    }
    IOBluetoothL2CAPChannel *channel=nil;
    IOReturn error=[self.device openL2CAPChannelSync:&channel withPSM:0x1001 delegate:self];
    self.channel=channel;
    printf("MAC_MINI_CHANNEL_INSTANCE channel=%p delegate=%p\n",(__bridge void *)channel,(__bridge void *)[channel delegate]); fflush(stdout);
    printf("MAC_MINI_L2CAP_OPEN status=%d paired=%d\n",error,[self.device isPaired]); fflush(stdout);
    if(error) { self.result=4; self.done=YES; return; }
    if(!self.payloadStarted) { self.payloadStarted=YES; [self performSelector:@selector(sendChallenge) withObject:nil afterDelay:0.1]; }
}
- (void)devicePairingUserConfirmationRequest:(id)sender numericValue:(BluetoothNumericValue)value {
    // The operator authorized pairing these two devices for this test.
    (void)value;
    [(IOBluetoothDevicePair *)sender replyUserConfirmation:YES];
}
- (void)devicePairingFinished:(id)sender error:(IOReturn)error {
    (void)sender;
    printf("MAC_MINI_PAIR_FINISHED status=%d paired=%d\n",error,[self.device isPaired]); fflush(stdout);
    if(error) { self.result=2; self.done=YES; return; }
    [self performSelector:@selector(startData) withObject:nil afterDelay:0.5];
}
- (void)l2capChannelData:(IOBluetoothL2CAPChannel *)channel data:(void *)bytes length:(size_t)length {
    (void)channel;
    NSData *response=[NSData dataWithBytes:bytes length:length];
    BOOL match=[response isEqualToData:self.challenge];
    if(!match) { self.result=6; self.done=YES; return; }
    self.received++;
    if(self.received < self.iterations) {
        [self performSelector:@selector(sendChallenge) withObject:nil afterDelay:0];
        return;
    }
    printf("MAC_MINI_BT_DATA_RESULT paired=%d packets=%lu bytes=%lu match=%d\n",[self.device isPaired],(unsigned long)self.received,(unsigned long)(length*self.received),match); fflush(stdout);
    self.result=0;
    self.done=YES;
}
- (void)l2capChannelOpenComplete:(IOBluetoothL2CAPChannel *)channel status:(IOReturn)error {
    printf("MAC_MINI_CHANNEL_OPEN_COMPLETE status=%d\n",error); fflush(stdout);
    if(error) { self.result=4; self.done=YES; }
    self.channel=channel;
}
- (void)l2capChannelClosed:(IOBluetoothL2CAPChannel *)channel { (void)channel; printf("MAC_MINI_CHANNEL_CLOSED\n"); fflush(stdout); }
@end
int main(int argc,const char **argv) {
 @autoreleasepool {
    if(argc<2 || argc>4) { fprintf(stderr,"usage: mac-mini-bt-test TARGET_ADDRESS [ITERATIONS [BYTES]]\n"); return 1; }
    [NSApplication sharedApplication];
    [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
    M5BTTest *test=[M5BTTest new];
    test.result=7;
    test.iterations=argc>2?strtoul(argv[2],NULL,10):1;
    NSUInteger size=argc>3?strtoul(argv[3],NULL,10):512;
    if(!test.iterations || test.iterations>10000 || size<96 || size>2000) return 1;
    test.device=[IOBluetoothDevice deviceWithAddressString:[NSString stringWithUTF8String:argv[1]]];
    if(!test.device || strcasecmp(argv[1], "C0:C7:DB:C7:D7:86")) return 1;
    NSString *nonce=[@"Gravity Bluetooth data proof " stringByAppendingString:[[NSUUID UUID] UUIDString]];
    NSMutableData *payload=[[nonce dataUsingEncoding:NSUTF8StringEncoding] mutableCopy];
    [payload setLength:size];
    test.challenge=payload;
    // Send only after the synchronous open call returns successfully.
    if([test.device isPaired]) [test performSelector:@selector(startData) withObject:nil afterDelay:0.5];
    else {
        test.pair=[IOBluetoothDevicePair pairWithDevice:test.device];
        test.pair.delegate=test;
        IOReturn error=[test.pair start];
        printf("MAC_MINI_PAIR_START status=%d\n",error); fflush(stdout);
        if(error) return 2;
    }
    test.deadline=[NSDate dateWithTimeIntervalSinceNow:300];
    [NSTimer scheduledTimerWithTimeInterval:0.1 target:test selector:@selector(tick:) userInfo:nil repeats:YES];
    [NSApp run];
    [test.channel closeChannel];
    [test.notification unregister];
    if(!test.done) fprintf(stderr,"MAC_MINI_BT_TIMEOUT\n");
    return test.result;
 }
}
