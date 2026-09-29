import 'package:test_runner/test_runner.dart';

void main() {
  defineTests(const [_weakRefs, _expandos, _finalizers, _baseUriAndPlatform]);
}

void _weakRefs(BaseResultCollector collector) {
  final object = Object();
  final ref = WeakReference(object);
  collector.recordBool(e: identical(ref.target, object));
  // A strongly-referenced object is never gone.
  collector.recordBool(e: ref.target == null);
}

void _expandos(BaseResultCollector collector) {
  final expando = Expando<String>('labels');
  final a = Object();
  final b = Object();
  collector.recordString(e: expando[a] ?? 'unset');
  expando[a] = 'A';
  expando[b] = 'B';
  collector.recordString(e: expando[a]!);
  collector.recordString(e: expando[b]!);
  expando[a] = 'A2';
  collector.recordString(e: expando[a]!);
  // Values on one target do not leak to another.
  collector.recordBool(e: expando[b] == 'B');
  collector.recordString(e: expando[Object()] ?? 'unset');
  // Setting null removes the association.
  expando[a] = null;
  collector.recordString(e: expando[a] ?? 'unset');
  collector.recordString(e: expando.name ?? '');
}

void _finalizers(BaseResultCollector collector) {
  var attached = false;
  final finalizer = Finalizer<String>((token) {
    attached = true;
  });
  final detachKey = Object();
  finalizer.attach(Object(), 'token');
  finalizer.attach(Object(), 'other', detach: detachKey);
  finalizer.detach(detachKey);
  // Without a GC cycle nothing has been finalized.
  collector.recordBool(e: attached);
}

void _baseUriAndPlatform(BaseResultCollector collector) {
  collector.recordBool(e: Uri.base.toString().isNotEmpty);
}
