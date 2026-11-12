// macOS DNS can first enter Foundation through Network/Security entitlement
// checks after fork. Complete NSNumber's lazy class initialization in the
// parent; class-cluster variants used by those checks must be ready as well.
#import <Foundation/Foundation.h>

int pyro_foundation_initialize_numbers(void) {
    @autoreleasepool {
        NSNumber *boolean = [NSNumber numberWithBool:YES];
        NSNumber *signedValue = [NSNumber numberWithLongLong:-1234567890123LL];
        NSNumber *unsignedValue = [NSNumber numberWithUnsignedLongLong:18446744073709551615ULL];
        NSNumber *floatingValue = [NSNumber numberWithDouble:1.25];
        return boolean.boolValue && signedValue.longLongValue == -1234567890123LL
            && unsignedValue.unsignedLongLongValue == 18446744073709551615ULL
            && floatingValue.doubleValue == 1.25;
    }
}
