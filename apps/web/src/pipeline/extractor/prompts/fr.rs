/// Versioned French extraction prompt (v1), mirroring `prompts::en`'s schema
/// exactly (IMP-REQ-007-01) so `RawExtraction` parses identically regardless
/// of source language. Bump the version comment and keep the old constant
/// around (e.g. `SYSTEM_PROMPT_V1`) if a future change needs to stay
/// reproducible against historical extractions.
pub const PROMPT_VERSION: &str = "fr-v1";

pub const SYSTEM_PROMPT: &str = r#"Vous extrayez des informations sur un projet de construction ou d'aménagement à partir d'un seul extrait d'un ordre du jour ou d'un procès-verbal d'une séance du conseil municipal.

Lisez l'extrait et remplissez chaque champ ci-dessous chaque fois que l'information est
littéralement présente dans le texte — ne laissez pas un champ vide (null) simplement parce que
son extraction semble incertaine; utilisez null uniquement lorsque l'information est
véritablement absente de l'extrait.

1. has_mention : si l'extrait décrit un projet de construction/d'aménagement, peu importe lequel.
2. physical_work : si l'extrait décrit un véritable projet de construction, de démolition ou de rénovation PHYSIQUE — par opposition à une affaire purement administrative, procédurale ou de rezonage sans travaux physiques décrits. Une modification de zonage ou un règlement de zonage qui NE décrit PAS également un projet physique précis (bâtiment/démolition) n'est PAS physical_work, même s'il permettra éventuellement un tel projet.
3. project_name : le nom du projet, s'il en est donné un (y compris les noms introduits par des expressions comme « connu sous le nom de », « appelé », ou entre guillemets).
4. civic_address : l'adresse civique (de rue), si elle apparaît n'importe où dans l'extrait.
5. project_type : le type de projet (p. ex. résidentiel, commercial, mixte, institutionnel, infrastructure, industriel), s'il est indiqué directement OU raisonnablement déductible de l'usage décrit (p. ex. « école », « hôpital », « bibliothèque » → institutionnel; « entrepôt » → industriel; « immeuble de bureaux » → commercial).
6. scale_units / scale_gfa_sqm / scale_storeys : indiquez chacun de ces trois éléments qui est mentionné — il est normal qu'un seul ou deux soient mentionnés; indiquez tous ceux qui sont présents.
7. reference_number : un numéro de référence explicite de demande, de permis ou de dossier, s'il est indiqué (p. ex. « Demande n° 2026-045 », « Permis #1234 ») — null si aucun n'est donné.
8. approval_status_raw : le statut d'approbation/de décision, copié exactement tel qu'écrit. Il s'agit très souvent d'une courte phrase ou d'un fragment autonome à la FIN de l'extrait, distinct de la description du projet — p. ex. un « Approuvé. », « Reporté. », « Reporté à la prochaine séance. » ou « Renvoyé au comité. » à la fin. Vérifiez TOUJOURS la dernière phrase de l'extrait pour ce champ, même si elle ne comporte qu'un ou deux mots, et remplissez ce champ chaque fois qu'un tel mot de décision/statut apparaît n'importe où dans l'extrait.

Si l'extrait ne décrit aucun projet, réglez has_mention à false et laissez tous les autres champs à null sauf physical_work (réglez-le à false).

Exemple d'extrait :
« Point 9 : Rénovation et agrandissement du centre communautaire institutionnel situé au 200, rue Elm, ajout de 2 étages. Approuvé. »

Extraction correcte pour cet exemple :
has_mention=true, physical_work=true, project_name=null (aucun donné), civic_address="200, rue Elm", project_type="institutionnel", scale_units=null, scale_gfa_sqm=null, scale_storeys=2, reference_number=null (aucun donné), approval_status_raw="Approuvé." — notez que la phrase finale d'un seul mot « Approuvé. » a été capturée même si elle est courte et distincte du reste de la description, et que l'adresse « 200, rue Elm » a été capturée même si elle apparaît en milieu de phrase plutôt qu'au début.

Répondez uniquement avec les champs JSON structurés demandés — n'ajoutez aucun commentaire."#;
