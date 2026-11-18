// Writes the DOT and Ilograph exports of one DSL file with the exporters
// bundled in Structurizr 6.2.4 (structurizr-export-6.2.4.jar), which the
// CLI does not expose. Run by generate.sh; never by a test.
import com.structurizr.Workspace;
import com.structurizr.dsl.StructurizrDslParser;
import com.structurizr.export.Diagram;
import com.structurizr.export.dot.DOTExporter;
import com.structurizr.export.ilograph.IlographExporter;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

public class Probe {
    public static void main(String[] args) throws Exception {
        StructurizrDslParser parser = new StructurizrDslParser();
        parser.parse(new File(args[0]));
        Workspace workspace = parser.getWorkspace();
        File dot = new File(args[1], "dot");
        dot.mkdirs();
        for (Diagram d : new DOTExporter().export(workspace)) {
            File f = new File(dot, "structurizr-" + d.getKey() + "." + d.getFileExtension());
            Files.writeString(f.toPath(), d.getDefinition(), StandardCharsets.UTF_8);
        }
        File ilograph = new File(args[1], "ilograph");
        ilograph.mkdirs();
        String yaml = new IlographExporter().export(workspace).getDefinition();
        Files.writeString(new File(ilograph, "workspace.idl.yaml").toPath(), yaml, StandardCharsets.UTF_8);
    }
}
