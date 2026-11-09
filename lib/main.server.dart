import 'package:jaspr/server.dart';
import 'app.dart' as app;

void main() {
  Jaspr.initializeApp();
  runApp(
    Document(
      title: 'Grindvakt — Secure dynamic DNS updater',
      body: const app.Grindvakt(),
    ),
  );
}