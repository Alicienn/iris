package fr.iris.mail;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.widget.RemoteViews;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

/**
 * Iris's Today widget. Iris writes what it shows, four lines, to `widget-today.txt` in
 * its files whenever Home changes, then asks for an update (`IrisActivity.updateWidgets`);
 * the widget only reads that file, so it needs no Iris running to be drawn.
 */
public class TodayWidget extends AppWidgetProvider {
    static final String FILE = "widget-today.txt";

    @Override
    public void onUpdate(Context context, AppWidgetManager manager, int[] ids) {
        String[] lines = read(context);
        for (int id : ids) {
            RemoteViews v = new RemoteViews(context.getPackageName(), R.layout.iris_today);
            v.setTextViewText(R.id.widget_date, lines[0]);
            v.setTextViewText(R.id.widget_line1, lines[1].isEmpty() ? "Open Iris" : lines[1]);
            v.setTextViewText(R.id.widget_line2, lines[2]);
            v.setTextViewText(R.id.widget_line3, lines[3]);
            Intent open = new Intent(context, IrisActivity.class)
                    .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            v.setOnClickPendingIntent(R.id.widget_root, PendingIntent.getActivity(context, 0,
                    open, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT));
            manager.updateAppWidget(id, v);
        }
    }

    /** Draws every Today widget on the home screens again. */
    static void refresh(Context context) {
        AppWidgetManager manager = AppWidgetManager.getInstance(context);
        int[] ids = manager.getAppWidgetIds(new ComponentName(context, TodayWidget.class));
        if (ids.length > 0) {
            new TodayWidget().onUpdate(context, manager, ids);
        }
    }

    private static String[] read(Context context) {
        String[] lines = {"", "", "", ""};
        File f = new File(context.getFilesDir(), FILE);
        try (BufferedReader r = new BufferedReader(new InputStreamReader(
                new FileInputStream(f), StandardCharsets.UTF_8))) {
            for (int i = 0; i < lines.length; i++) {
                String l = r.readLine();
                if (l == null) {
                    break;
                }
                lines[i] = l;
            }
        } catch (Exception e) {
            // Nothing written yet: the widget asks to open Iris.
        }
        return lines;
    }
}
