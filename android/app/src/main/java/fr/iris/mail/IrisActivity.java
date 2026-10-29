package fr.iris.mail;

import android.Manifest;
import android.app.NativeActivity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.PowerManager;
import android.provider.OpenableColumns;
import android.provider.Settings;

import com.google.android.gms.common.moduleinstall.ModuleInstall;
import com.google.android.gms.common.moduleinstall.ModuleInstallClient;
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest;
import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.codescanner.GmsBarcodeScanner;
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions;
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Android's NativeActivity, which loads libiris_android.so, with the things Iris asks of
 * Java: a QR code read by Google's scanner, the link a QR code opened Iris with (a
 * pairing code read by the phone's own camera), and files chosen in the system's
 * document picker. Rust asks for each through JNI and reads the answers back the same
 * way: no native callback to register.
 */
public class IrisActivity extends NativeActivity {
    private static final int PICK_FILES = 7101;
    private static final int ASK_NOTIFICATIONS = 7102;
    private static final String CHANNEL = "iris";
    /** The answer to the notifications' question: null while Android asks. */
    private volatile String asked;

    /**
     * The scanner's answer: null while it is open or was never opened, "" when closed
     * without a code, U+0001 then the reason when the camera could not be used.
     */
    private volatile String scanned;
    /** The files picked, one path a line: null while the picker is open. */
    private volatile String picked;

    /**
     * Iris is already open when the camera hands it a pairing link: the new intent
     * becomes the activity's, where `takeLink` finds it.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }

    /**
     * Opens Google's code scanner over Iris. It needs no camera permission of Iris's
     * own: the camera is Play services', and only the code read comes back. Its screen
     * is a module of Play services, fetched first when the phone does not have it yet.
     */
    public void scanQr() {
        scanned = null;
        runOnUiThread(() -> {
            try {
                GmsBarcodeScannerOptions options = new GmsBarcodeScannerOptions.Builder()
                        .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                        .build();
                GmsBarcodeScanner scanner = GmsBarcodeScanning.getClient(this, options);
                ModuleInstallClient modules = ModuleInstall.getClient(this);
                modules.areModulesAvailable(scanner)
                        .addOnSuccessListener(r -> {
                            if (r.areModulesAvailable()) {
                                startScan(scanner);
                            } else {
                                modules.installModules(ModuleInstallRequest.newBuilder()
                                                .addApi(scanner)
                                                .build())
                                        .addOnSuccessListener(x -> startScan(scanner))
                                        .addOnFailureListener(this::failed);
                            }
                        })
                        .addOnFailureListener(this::failed);
            } catch (RuntimeException e) {
                failed(e);
            }
        });
    }

    private void startScan(GmsBarcodeScanner scanner) {
        scanner.startScan()
                .addOnSuccessListener(code -> {
                    String text = code.getRawValue();
                    scanned = text == null ? "" : text;
                })
                .addOnCanceledListener(() -> scanned = "")
                .addOnFailureListener(this::failed);
    }

    private void failed(Exception e) {
        String why = e == null ? null : e.getMessage();
        scanned = "\u0001" + (why == null ? "the camera could not be opened" : why);
    }

    /** The code read, once: see `scanned`. */
    public String takeScan() {
        String text = scanned;
        if (text != null) {
            scanned = null;
        }
        return text;
    }

    /** The link Iris was opened with, once; null when there is none. */
    public String takeLink() {
        Intent intent = getIntent();
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())
                || intent.getData() == null) {
            return null;
        }
        String link = intent.getData().toString();
        setIntent(new Intent(Intent.ACTION_MAIN));
        return link;
    }

    /** The system's document picker, for one file or several. */
    public void pickFiles(boolean imagesOnly) {
        picked = null;
        runOnUiThread(() -> {
            try {
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType(imagesOnly ? "image/*" : "*/*");
                intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                startActivityForResult(intent, PICK_FILES);
            } catch (RuntimeException e) {
                picked = "";
            }
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != PICK_FILES) {
            return;
        }
        if (resultCode != RESULT_OK || data == null) {
            picked = "";
            return;
        }
        // Copied off the main thread: a large file must not freeze the screen.
        new Thread(() -> {
            StringBuilder paths = new StringBuilder();
            if (data.getClipData() != null) {
                for (int i = 0; i < data.getClipData().getItemCount(); i++) {
                    append(paths, copy(data.getClipData().getItemAt(i).getUri()));
                }
            } else if (data.getData() != null) {
                append(paths, copy(data.getData()));
            }
            picked = paths.toString();
        }).start();
    }

    private static void append(StringBuilder paths, String path) {
        if (path != null) {
            paths.append(path).append('\n');
        }
    }

    /** A document copied into the cache under its own name; null when it cannot be read. */
    private String copy(Uri uri) {
        String name = "attachment";
        try (Cursor c = getContentResolver().query(uri, null, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int i = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (i >= 0 && c.getString(i) != null) {
                    name = c.getString(i).replace('/', '_');
                }
            }
        } catch (RuntimeException e) {
            // The name is a nicety: the bytes are what matter.
        }
        File folder = new File(getCacheDir(), "picked/" + System.nanoTime());
        if (!folder.mkdirs()) {
            return null;
        }
        File out = new File(folder, name);
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream o = new FileOutputStream(out)) {
            if (in == null) {
                return null;
            }
            byte[] buffer = new byte[64 * 1024];
            int n;
            while ((n = in.read(buffer)) > 0) {
                o.write(buffer, 0, n);
            }
            return out.getAbsolutePath();
        } catch (Exception e) {
            return null;
        }
    }

    /** The files picked, once: see `picked`. */
    public String takePicked() {
        String text = picked;
        if (text != null) {
            picked = null;
        }
        return text;
    }

    /** The Today widgets drawn again from the file Iris just wrote. */
    public void updateWidgets() {
        runOnUiThread(() -> TodayWidget.refresh(this));
    }

    // --- Notifications and the battery ---

    /** Whether Iris may show notifications (asked for since Android 13). */
    public boolean notificationsAllowed() {
        NotificationManager nm = getSystemService(NotificationManager.class);
        return nm != null && nm.areNotificationsEnabled();
    }

    /**
     * Asks for the notifications. Android shows its question once or twice; after that,
     * or before Android 13, the answer is what it already was. "1" or "0" in `asked`.
     */
    public void askNotifications() {
        asked = null;
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT >= 33
                    && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                    != PackageManager.PERMISSION_GRANTED) {
                requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS},
                        ASK_NOTIFICATIONS);
            } else {
                asked = notificationsAllowed() ? "1" : "0";
            }
        });
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions,
            int[] results) {
        super.onRequestPermissionsResult(requestCode, permissions, results);
        if (requestCode == ASK_NOTIFICATIONS) {
            asked = notificationsAllowed() ? "1" : "0";
        }
    }

    /** The answer to `askNotifications`, once; null while Android is asking. */
    public String takeAsked() {
        String text = asked;
        if (text != null) {
            asked = null;
        }
        return text;
    }

    /** Iris's page of notifications in the system's settings. */
    public void openNotificationSettings() {
        runOnUiThread(() -> {
            Intent intent = new Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(Settings.EXTRA_APP_PACKAGE, getPackageName());
            open(intent, new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                    Uri.parse("package:" + getPackageName())));
        });
    }

    /** Android leaves Iris running behind the screen: no battery optimisation on it. */
    public boolean backgroundAllowed() {
        PowerManager pm = getSystemService(PowerManager.class);
        return pm != null && pm.isIgnoringBatteryOptimizations(getPackageName());
    }

    /** Android's question to stop optimising Iris's battery, or its list where it is. */
    public void openBatterySettings() {
        runOnUiThread(() -> open(
                new Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                        Uri.parse("package:" + getPackageName())),
                new Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)));
    }

    private void open(Intent first, Intent otherwise) {
        try {
            startActivity(first);
        } catch (RuntimeException e) {
            try {
                startActivity(otherwise);
            } catch (RuntimeException ignored) {
                // Neither page exists on this phone: nothing to open.
            }
        }
    }

    /**
     * A notification of Iris's: new mail, a reminder. Its id replaces an older one with
     * the same; a tap brings Iris forward.
     */
    public void notify(int id, String title, String text) {
        NotificationManager nm = getSystemService(NotificationManager.class);
        if (nm == null) {
            return;
        }
        if (nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Mail and reminders",
                    NotificationManager.IMPORTANCE_DEFAULT));
        }
        Intent back = new Intent(this, IrisActivity.class)
                .setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        PendingIntent tap = PendingIntent.getActivity(this, 0, back,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        Notification n = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_dialog_email)
                .setContentTitle(title)
                .setContentText(text)
                .setStyle(new Notification.BigTextStyle().bigText(text))
                .setContentIntent(tap)
                .setAutoCancel(true)
                .build();
        try {
            nm.notify(id, n);
        } catch (SecurityException e) {
            // Notifications refused since: nothing shows, nothing breaks.
        }
    }
}
