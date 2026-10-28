package fr.iris.mail;

import android.app.NativeActivity;
import android.content.Intent;

import com.google.mlkit.vision.barcode.common.Barcode;
import com.google.mlkit.vision.codescanner.GmsBarcodeScanner;
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions;
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning;

/**
 * Android's NativeActivity, which loads libiris_android.so, with the two things Iris
 * asks of Java: a QR code read by Google's scanner, and the link a QR code opened Iris
 * with (a pairing code read by the phone's own camera). Rust asks for both through JNI
 * and reads the answers back the same way: no native callback to register.
 */
public class IrisActivity extends NativeActivity {
    /** The scanner's answer: null while it is open or was never opened. */
    private volatile String scanned;

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
     * Opens Google's code scanner over Iris. It needs no camera permission: the camera
     * is Play services', and only the code read comes back.
     */
    public void scanQr() {
        scanned = null;
        runOnUiThread(() -> {
            try {
                GmsBarcodeScannerOptions options = new GmsBarcodeScannerOptions.Builder()
                        .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                        .build();
                GmsBarcodeScanner scanner = GmsBarcodeScanning.getClient(this, options);
                scanner.startScan()
                        .addOnSuccessListener(code -> {
                            String text = code.getRawValue();
                            scanned = text == null ? "" : text;
                        })
                        .addOnCanceledListener(() -> scanned = "")
                        .addOnFailureListener(e -> scanned = "");
            } catch (RuntimeException e) {
                scanned = "";
            }
        });
    }

    /**
     * The code read, once: null while the scanner is open, "" when it was closed or
     * could not open.
     */
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
}
