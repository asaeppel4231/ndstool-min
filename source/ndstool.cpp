/*
	Nintendo DS rom tool
	by Rafael Vuijk (aka DarkFader)
*/

#include <unistd.h>
#include "ndstool.h"
#include "ndscreate.h"
#include "ndsextract.h"
#include "encryption.h"

/*
 * Variables
 */
int verbose = 0;
Header header;
FILE *fNDS = 0;
char *romlistfilename = 0;
char *filemasks[MAX_FILEMASKS];
int filemask_num = 0;
char *ndsfilename = 0;
char *arm7filename = 0;
char *arm9filename = 0;
char *arm7ifilename = 0;
char *arm9ifilename = 0;
char *filerootdir = 0;
char *overlaydir = 0;
char *arm7ovltablefilename = 0;
char *arm9ovltablefilename = 0;
char *headerfilename_or_size = 0;
//char *uniquefilename = 0;
char *title = 0;
char *makercode = 0;
char *gamecode = 0;
char *vhdfilename = 0;
char *sramfilename = 0;
int latency1 = 0x1FFF;	//0x8F8;
int latency2 = 0x3F;	//0x18;
unsigned int romversion = 0;
char endecrypt_option = 0;

unsigned int arm9RamAddress = 0;
unsigned int arm7RamAddress = 0;
unsigned int arm9Entry = 0;
unsigned int arm7Entry = 0;
unsigned int titleidHigh = 0x00030000; // DSi-enhanced gamecard. 0x00030004 (DSiWare) cannot be loaded as a card from DSi Menu
unsigned int scfgExtMask = 0x80040407; // enable access to everything
unsigned int accessControl = 0x00000138;
unsigned int mbkArm7WramMapAddress = 0;
unsigned int appFlags = 0x01;
unsigned int deviceListRamAddress = 0x03800000;


#define REQUIRED(var)	var = ((argc > a+1) ? argv[++a] : 0)										// must precede OPTIONAL
#define OPTIONAL(var)	{ /*fprintf(stderr, "'%s'\n", argv[a]);*/ char *t = ((argc > a+1) && (argv[a+1][0] != '-') ? argv[++a] : 0); if (!var) var = t; else if (t) fprintf(stderr, "%s is already specified!\n", #var); }		// final paramter requirement checks are done when performing actions
#define OPTIONAL_INT(var)	{ char *t = ((argc > a+1) && (argv[a+1][0] != '-') ? argv[++a] : 0); if (t) var = strtoul(t,0,0); }		// like OPTIONAL, but for (positive) integers
#define MAX_ACTIONS		32
#define ADDACTION(a)	{ if (num_actions < MAX_ACTIONS) actions[num_actions++] = a; }

enum {
	ACTION_SHOWINFO,
	ACTION_FIXHEADERCRC,
	ACTION_ENCRYPTSECUREAREA,
	ACTION_LISTFILES,
	ACTION_EXTRACT,
	ACTION_CREATE,
};

/*
 * main
 */
int main(int argc, char *argv[])
{
	#ifdef _NDSTOOL_P_H
		if (sizeof(Header) != 0x200) { fprintf(stderr, "Header size %d != %d\n", sizeof(Header), 0x200); return 1; }
	#endif

	if (argc < 2) return 0;
	
	int num_actions = 0;
	int actions[MAX_ACTIONS];

	/*
	 * parse parameters to actions
	 */

	for (int a=1; a<argc; a++)
	{
		if (argv[a][0] == '-')
		{
			switch (argv[a][1])
			{
				case 'i':	// show information
				{
					ADDACTION(ACTION_SHOWINFO);
					OPTIONAL(ndsfilename);
					break;
				}

				case 'f':	// fix header CRC
				{
					ADDACTION(ACTION_FIXHEADERCRC);
					OPTIONAL(ndsfilename);
					break;
				}

				case 's':	// en-/decrypt secure area
				{
					ADDACTION(ACTION_ENCRYPTSECUREAREA);
					endecrypt_option = argv[a][2];
					OPTIONAL(ndsfilename);
					break;
				}

				case 'l':	// list files
				{
					ADDACTION(ACTION_LISTFILES);
					OPTIONAL(ndsfilename);
					break;
				}

				case 'x':	// extract
				{
					ADDACTION(ACTION_EXTRACT);
					OPTIONAL(ndsfilename);
					break;
				}
				
				case 'w':	// wildcard filemasks
				{
					while (1)
					{
						char *filemask = 0;
						OPTIONAL(filemask);
						if (!(filemasks[filemask_num] = filemask)) break;
						if (++filemask_num >= MAX_FILEMASKS) return 1;
					}
					break;
				}

				case 'c':	// create
				{
					ADDACTION(ACTION_CREATE);
					OPTIONAL(ndsfilename);
					break;
				}

				// file root directory
				case 'd': REQUIRED(filerootdir); break;

				// ARM7 filename
				case '7': 
					if (argv[a][2] == 'i') {
						REQUIRED(arm7ifilename);
					} else {
						REQUIRED(arm7filename);
					}
					break;
				// ARM9 filename
				case '9':
					if (argv[a][2] == 'i') {
						REQUIRED(arm9ifilename);
					} else {
						REQUIRED(arm9filename);
					}
					break;

				case 'h':	// load header or header size
					REQUIRED(headerfilename_or_size);
					break;

				/*case 'u':	// unique ID file
					REQUIRED(uniquefilename);
					break;*/

				case 'u': // DSi title ID high word
					if (argc > a)
						titleidHigh = strtoul(argv[++a], 0, 16);
					break;

				case 'z': // SCFG access flags
					if (argc > a)
						scfgExtMask = strtoul(argv[++a], 0, 16);
					break;

				case 'a': // DSi access control flags
					if (argc > a)
						accessControl = strtoul(argv[++a], 0, 16);
					break;

				case 'p': // DSi application flags
					if (argc > a)
						appFlags = strtoul(argv[++a], 0, 16) & 0xFF;
					break;

				case 'q': // DSi ARM7 WRAM_A map address
					if (argc > a)
						mbkArm7WramMapAddress = strtoul(argv[++a], 0, 16);
					break;

				case 'v':	// verbose
					for (char *p=argv[a]; *p; p++) if (*p == 'v') verbose++;
					OPTIONAL(romlistfilename);
					break;

				case 'n':	// latency
					//compatibility = true;
					OPTIONAL_INT(latency1);
					OPTIONAL_INT(latency2);
					break;

				case 'r':	// RAM address
					switch (argv[a][2])
					{
						case '7': arm7RamAddress = (argc > a) ? strtoul(argv[++a], 0, 0) : 0; break;
						case '9': arm9RamAddress = (argc > a) ? strtoul(argv[++a], 0, 0) : 0; break;
						default: return 1;
					}
					break;

				case 'e':	// entry point
					switch (argv[a][2])
					{
						case '7': arm7Entry = (argc > a) ? strtoul(argv[++a], 0, 0) : 0; break;
						case '9': arm9Entry = (argc > a) ? strtoul(argv[++a], 0, 0) : 0; break;
						default: return 1;
					}
					break;

				case 'm':	// maker code
					REQUIRED(makercode);
					break;

				case 'g':	// game code
					REQUIRED(gamecode);
					OPTIONAL(makercode);
					OPTIONAL(title);
					OPTIONAL_INT(romversion);
					break;

				case 'y':	// overlay table file / directory
					switch (argv[a][2])
					{
						case '7': REQUIRED(arm7ovltablefilename); break;
						case '9': REQUIRED(arm9ovltablefilename); break;
						case 0: REQUIRED(overlaydir); break;
						default: return 1;
					}
					break;

				case '?':
					return 0;

				default:
					return 1;
			}
		}
		else
		{
			if (ndsfilename)
			{
				fprintf(stderr, "NDS filename is already given!\n");
				return 1;
			}
			ndsfilename = argv[a];
			break;
		}
	}

	/*
	 * sanity checks
	 */

	if (gamecode)
	{
		if (strlen(gamecode) != 4)
		{
			fprintf(stderr, "Game code must be 4 characters!\n");
			return 1;
		}
		for (int i=0; i<4; i++) if ((gamecode[i] >= 'a') && (gamecode[i] <= 'z'))
		{
			fprintf(stderr, "Warning: Gamecode contains lowercase characters.\n");
			break;
		}
		if (gamecode[0] == 'A')
		{
			fprintf(stderr, "Warning: Gamecode starts with 'A', which might be used for another commercial product.\n");
		}
	}
	if (makercode && (strlen(makercode) != 2))
	{
		fprintf(stderr, "Maker code must be 2 characters!\n");
		return 1;
	}
	if (title && (strlen(title) > 12))
	{
		fprintf(stderr, "Title can be no more than 12 characters!\n");
		return 1;
	}
	if (romversion > 255) {
		fprintf(stderr, "romversion can only be 0 - 255!\n");
		return 1;
	}

	/*
	 * perform actions
	 */

	int status = 0;
	for (int i=0; i<num_actions; i++)
	{
//printf("action %d\n", actions[i]);
		switch (actions[i])
		{
			case ACTION_SHOWINFO:
				ShowInfo(ndsfilename);
				break;

			case ACTION_FIXHEADERCRC:
				FixHeaderCRC(ndsfilename);
				break;

			case ACTION_EXTRACT: {
				unsigned int headersize = 0x200;
				fNDS = fopen(ndsfilename, "rb");
				if (!fNDS) { fprintf(stderr, "Cannot open file '%s'.\n", ndsfilename); exit(1); }
				fread(&header, 1, headersize, fNDS);
				if (header.unitcode & 2) { // DSi application
					fread((char*)&header + headersize, 1, sizeof(Header) - headersize, fNDS);
					headersize = sizeof(Header);
				}
				fclose(fNDS);

				if (arm9filename) Extract(arm9filename, true, 0x20, true, 0x2C, true);
				if (arm7filename) Extract(arm7filename, true, 0x30, true, 0x3C);
				if (header.unitcode & 2) {
					if (arm9ifilename) Extract(arm9ifilename, true, 0x1C0, true, 0x1CC, true);
					if (arm7ifilename) Extract(arm7ifilename, true, 0x1D0, true, 0x1DC);
				}
				if (headerfilename_or_size) Extract(headerfilename_or_size, false, 0x0, false, headersize);
				if (arm9ovltablefilename) Extract(arm9ovltablefilename, true, 0x50, true, 0x54);
				if (arm7ovltablefilename) Extract(arm7ovltablefilename, true, 0x58, true, 0x5C);
				if (overlaydir) ExtractOverlayFiles();
				if (filerootdir) ExtractFiles(ndsfilename);
				break;
			}

			case ACTION_CREATE:
				Create();
				break;

			case ACTION_LISTFILES:
				filerootdir = 0;
				/*status =*/ ExtractFiles(ndsfilename);
				break;
			
			case ACTION_ENCRYPTSECUREAREA:
			{
				/*status =*/ EnDecryptSecureArea(ndsfilename, endecrypt_option);
				break;
			}
		}
	}

	return (status < 0) ? 1 : 0;
}
