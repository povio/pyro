#import <Foundation/Foundation.h>
#import <CoreML/CoreML.h>
#import <math.h>

static void check(BOOL condition, NSString *message) {
    if (!condition) { NSLog(@"%@", message); exit(1); }
}

int main(int argc, const char **argv) {
  @autoreleasepool {
    if (argc != 2) return 2;
    NSError *error = nil;
    NSURL *source = [NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[1]]];
    NSTimeInterval start = NSProcessInfo.processInfo.systemUptime;
    NSURL *compiled = [MLModel compileModelAtURL:source error:&error];
    check(compiled != nil, error.description);
    NSTimeInterval compiledAt = NSProcessInfo.processInfo.systemUptime;

    MLModelConfiguration *cfg = [MLModelConfiguration new];
    cfg.computeUnits = MLComputeUnitsCPUOnly;
    MLModel *model = [MLModel modelWithContentsOfURL:compiled configuration:cfg error:&error];
    check(model != nil, error.description);
    check(model.configuration.computeUnits == MLComputeUnitsCPUOnly, @"CPU-only configuration lost");
    MLFeatureDescription *inputFeature = model.modelDescription.inputDescriptionsByName[@"x"];
    MLFeatureDescription *outputFeature = model.modelDescription.outputDescriptionsByName[@"y"];
    check([inputFeature.multiArrayConstraint.shape isEqual:@[@2]], @"unexpected input shape");
    check([outputFeature.multiArrayConstraint.shape isEqual:@[@3]], @"unexpected output shape");
    check(inputFeature.multiArrayConstraint.dataType == MLMultiArrayDataTypeFloat32, @"wrong input type");
    check(outputFeature.multiArrayConstraint.dataType == MLMultiArrayDataTypeFloat32, @"wrong output type");

    MLMultiArray *x = [[MLMultiArray alloc] initWithShape:@[@2] dataType:MLMultiArrayDataTypeFloat32 error:&error];
    check(x != nil, error.description);
    MLFeatureValue *value = [MLFeatureValue featureValueWithMultiArray:x];
    check([inputFeature isAllowedValue:value], @"input descriptor rejected matching tensor");
    MLMultiArray *other = [[MLMultiArray alloc] initWithShape:@[@3] dataType:MLMultiArrayDataTypeFloat32 error:&error];
    check(![inputFeature isAllowedValue:[MLFeatureValue featureValueWithMultiArray:other]], @"wrong shape accepted");
    MLDictionaryFeatureProvider *inputs = [[MLDictionaryFeatureProvider alloc] initWithDictionary:@{@"x":value} error:&error];
    check(inputs != nil,error.description);

    const float cases[][2]={{2,3},{-1,2},{1,-2}};
    const float expected[][3]={{7,1.5,0},{2,3.5,0},{0,0,6.5}};
    NSMutableArray *actual = [NSMutableArray new];
    for (size_t n = 0; n < 3; n++) {
      x[0]=@(cases[n][0]); x[1]=@(cases[n][1]);
      id<MLFeatureProvider> result=[model predictionFromFeatures:inputs error:&error];
      check(result != nil,error.description);
      MLMultiArray *y=[result featureValueForName:@"y"].multiArrayValue;
      check(y.count == 3 && y.dataType == MLMultiArrayDataTypeFloat32, @"bad output");
      NSMutableArray *row=[NSMutableArray new];
      for (NSUInteger i=0;i<3;i++) {
        check(fabs(y[i].floatValue-expected[n][i]) < 1e-6, @"prediction mismatch");
        [row addObject:y[i]];
      }
      [actual addObject:row];
    }
    NSDictionary *report=@{@"model":@"affine-plus-ReLU", @"computeUnits":@"CPUOnly", @"compiledOnce":@YES,
      @"inputShape":inputFeature.multiArrayConstraint.shape, @"outputShape":outputFeature.multiArrayConstraint.shape,
      @"wrongShapeRejected":@YES, @"predictions":actual, @"compileMs":@((compiledAt-start)*1000)};
    NSData *json=[NSJSONSerialization dataWithJSONObject:report options:0 error:&error];
    check(json != nil,error.description);
    fwrite(json.bytes,1,json.length,stdout); puts("");
    [[NSFileManager defaultManager] removeItemAtURL:compiled error:nil];
  }
  return 0;
}
